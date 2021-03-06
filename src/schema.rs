use std::cmp;
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use kudu_pb::common::{ColumnSchemaPB, SchemaPB};
#[cfg(any(feature="quickcheck", test))] use quickcheck;

use CompressionType;
use DataType;
use EncodingType;
use Error;
use Result;
use Row;

/// `Column` instances hold metadata information about columns in a Kudu table.
///
/// `Column` also serves as a builder object for specifying new columns during create and alter
/// table operations.
#[derive(Clone, PartialEq, Eq)]
pub struct Column {
    name: String,
    data_type: DataType,
    is_nullable: bool,
    compression: CompressionType,
    encoding: EncodingType,
    block_size: u32,
}

impl Column {

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn data_type(&self) -> DataType {
        self.data_type
    }

    pub fn is_nullable(&self) -> bool {
        self.is_nullable
    }

    pub fn encoding(&self) -> EncodingType {
        self.encoding
    }

    pub fn compression(&self) -> CompressionType {
        self.compression
    }

    pub fn block_size(&self) -> Option<u32> {
        if self.block_size <= 0 {
            None
        } else {
            Some(self.block_size)
        }
    }

    /// Returns a new column builder.
    pub fn builder<S>(name: S, data_type: DataType) -> Column where S: Into<String> {
        Column {
            name: name.into(),
            data_type: data_type,
            is_nullable: true,
            compression: CompressionType::Default,
            encoding: EncodingType::Auto,
            block_size: 0,
        }
    }

    pub fn set_nullable(mut self) -> Column {
        self.set_nullable_by_ref();
        self
    }

    pub fn set_nullable_by_ref(&mut self) -> &mut Column {
        self.is_nullable = true;
        self
    }

    pub fn set_not_null(mut self) -> Column {
        self.set_not_null_by_ref();
        self
    }

    pub fn set_not_null_by_ref(&mut self) -> &mut Column {
        self.is_nullable = false;
        self
    }

    pub fn set_encoding(mut self, encoding: EncodingType) -> Column {
        self.set_encoding_by_ref(encoding);
        self
    }

    pub fn set_encoding_by_ref(&mut self, encoding: EncodingType) -> &mut Column {
        self.encoding = encoding;
        self
    }

    pub fn set_compression(mut self, compression: CompressionType) -> Column {
        self.set_compression_by_ref(compression);
        self
    }

    pub fn set_compression_by_ref(&mut self, compression: CompressionType) -> &mut Column {
        self.compression = compression;
        self
    }

    pub fn set_block_size(mut self, block_size: u32) -> Column {
        self.set_block_size_by_ref(block_size);
        self
    }

    pub fn set_block_size_by_ref(&mut self, block_size: u32) -> &mut Column {
        self.block_size = block_size;
        self
    }

    #[doc(hidden)]
    pub fn to_pb(&self, is_key: bool) -> ColumnSchemaPB {
        let mut pb = ColumnSchemaPB::new();
        pb.set_name(self.name.clone());
        pb.set_field_type(self.data_type.to_pb());
        pb.set_is_nullable(self.is_nullable);
        pb.set_is_key(is_key);
        pb.set_encoding(self.encoding.to_pb());
        pb.set_compression(self.compression.to_pb());
        // TODO: checked cast.
        pb.set_cfile_block_size(self.block_size as i32);
        pb
    }

    #[doc(hidden)]
    pub fn from_pb(mut pb: ColumnSchemaPB) -> Result<Column> {
        Ok(Column {
            name: pb.take_name(),
            data_type: try!(DataType::from_pb(pb.get_field_type())),
            is_nullable: pb.get_is_nullable(),
            compression: try!(CompressionType::from_pb(pb.get_compression())),
            encoding: try!(EncodingType::from_pb(pb.get_encoding())),
            // TODO: checked cast.
            block_size: pb.get_cfile_block_size() as u32,
        })
    }
}

impl fmt::Debug for Column {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        try!(write!(f, "{} {:?}", self.name, self.data_type));
        if !self.is_nullable {
            try!(write!(f, " NOT NULL"));
        }
        if self.encoding != EncodingType::Auto {
            try!(write!(f, " ENCODING {:?}", self.encoding));
        }
        if self.compression != CompressionType::Default {
            try!(write!(f, " COMPRESSION {:?}", self.compression));
        }
        if let Some(block_size) = self.block_size() {
            try!(write!(f, " BLOCK SIZE {}", block_size));
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
struct Inner {
    columns: Vec<Column>,
    columns_by_name: HashMap<String, usize>,
    column_offsets: Vec<usize>,
    num_primary_key_columns: usize,
    row_size: usize,
    has_nullable_columns: bool,
}

#[derive(Clone)]
pub struct Schema {
    inner: Arc<Inner>,
}

impl Schema {

    fn new(columns: Vec<Column>, num_primary_key_columns: usize) -> Schema {
        let mut columns_by_name = HashMap::with_capacity(columns.len());
        let mut column_offsets = Vec::with_capacity(columns.len());
        let mut row_size = 0;
        let mut has_nullable_columns = false;
        for (idx, column) in columns.iter().enumerate() {
            columns_by_name.insert(column.name().to_string(), idx);
            column_offsets.push(row_size);
            row_size += column.data_type.size();
            has_nullable_columns |= column.is_nullable();
        }

        Schema {
            inner: Arc::new(Inner {
                columns: columns,
                columns_by_name: columns_by_name,
                column_offsets: column_offsets,
                num_primary_key_columns: num_primary_key_columns,
                row_size: row_size,
                has_nullable_columns: has_nullable_columns,
            })
        }
    }

    pub fn columns(&self) -> &[Column] {
        &self.inner.columns
    }

    pub fn column(&self, index: usize) -> Option<&Column> {
        self.inner.columns.get(index)
    }

    pub fn column_by_name(&self, name: &str) -> Option<&Column> {
        self.column_index(name).map(|idx| &self.inner.columns[idx])
    }

    pub fn column_index(&self, name: &str) -> Option<usize> {
        self.inner.columns_by_name.get(name).cloned()
    }

    pub fn primary_key(&self) -> &[Column] {
        &self.inner.columns[0..self.inner.num_primary_key_columns]
    }

    #[doc(hidden)]
    pub fn num_primary_key_columns(&self) -> usize {
        self.inner.num_primary_key_columns
    }

    pub fn primary_key_projection(&self) -> Schema {
        Schema::new(self.primary_key().to_owned(), self.num_primary_key_columns())
    }

    pub fn row_size(&self) -> usize {
        self.inner.row_size
    }

    pub fn has_nullable_columns(&self) -> bool {
        self.inner.has_nullable_columns
    }

    pub fn column_offsets(&self) -> &[usize] {
        &self.inner.column_offsets
    }

    pub fn new_row(&self) -> Row {
        Row::new(self.clone())
    }

    pub fn ref_eq(&self, other: &Schema) -> bool {
        let this: *const Inner = &*self.inner;
        let that: *const Inner = &*other.inner;
        this == that
    }

    #[doc(hidden)]
    pub fn as_pb(&self) -> SchemaPB {
        let mut pb = SchemaPB::new();
        for (idx, column) in self.inner.columns.iter().enumerate() {
            pb.mut_columns().push(column.to_pb(idx < self.inner.num_primary_key_columns));
        }
        pb
    }

    #[doc(hidden)]
    pub fn from_pb(mut pb: SchemaPB) -> Result<Schema> {
        let mut num_primary_key_columns = 0;
        let mut columns = Vec::with_capacity(pb.get_columns().len());
        for column in pb.take_columns().into_iter() {
            if column.get_is_key() { num_primary_key_columns += 1 }
            columns.push(try!(Column::from_pb(column)))
        }
        Ok(Schema::new(columns, num_primary_key_columns))
    }
}

impl cmp::PartialEq for Schema {
    fn eq(&self, other: &Schema) -> bool {
        self.ref_eq(other) ||
            (self.inner.num_primary_key_columns == other.inner.num_primary_key_columns &&
             self.inner.columns == other.inner.columns)
    }
}

impl cmp::Eq for Schema { }

impl fmt::Debug for Schema {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        try!(write!(f, "("));
        let mut is_first = true;
        for column in self.columns() {
            if is_first {
                is_first = false;
                try!(write!(f, "{:?}", column));
            } else {
                try!(write!(f, ", {:?}", column));
            }
        }
        try!(write!(f, ") PRIMARY KEY ("));
        is_first = true;
        for column in self.primary_key() {
            if is_first {
                is_first = false;
                try!(write!(f, "{}", column.name()));
            } else {
                try!(write!(f, ", {}", column.name()));
            }
        }
        write!(f, ")")
    }
}

#[cfg(any(feature="quickcheck", test))]
impl quickcheck::Arbitrary for Schema {
    fn arbitrary<G>(g: &mut G) -> Schema where G: quickcheck::Gen {
        use std::collections::HashSet;

        let mut builder = SchemaBuilder::new();

        let mut primary_key_columns: HashSet<String> = HashSet::arbitrary(g);
        while primary_key_columns.is_empty() {
            primary_key_columns = HashSet::arbitrary(g);
        }

        let mut columns: HashSet<String> = HashSet::arbitrary(g);
        while !primary_key_columns.is_disjoint(&columns) {
            columns = HashSet::arbitrary(g);
        }

        let mut columns = primary_key_columns.union(&columns).collect::<Vec<_>>();
        g.shuffle(&mut columns);

        for column in columns {
            let is_pk = primary_key_columns.contains(column);
            let data_type = if is_pk { DataType::arbitrary_primary_key(g) }
                                else { DataType::arbitrary(g) };


            let mut column = Column::builder(column.as_str(), data_type);

            if is_pk || bool::arbitrary(g) { column.set_not_null_by_ref() }
            else { column.set_nullable_by_ref() };

            column.set_encoding_by_ref(EncodingType::arbitrary(g, data_type));
            column.set_compression_by_ref(CompressionType::arbitrary(g));
            if bool::arbitrary(g) {
                // TODO: can Kudu support arbitrary block sizes?
                column.set_block_size_by_ref(u32::arbitrary(g));
            }
            builder.add_column_by_ref(column);
        }

        let mut primary_key_columns: Vec<String> = primary_key_columns.iter().cloned().collect();
        g.shuffle(&mut primary_key_columns);

        builder.set_primary_key_by_ref(primary_key_columns);
        builder.build().unwrap()
    }

    /// Returns an iterator containing versions of the schema with columns removed.
    fn shrink(&self) -> Box<Iterator<Item=Self>> {
        if self.columns().len() == 1 { return quickcheck::empty_shrinker(); }

        let mut schemas: Vec<Schema> = Vec::new();
        let start_idx = if self.num_primary_key_columns() > 1 { 0 } else { 1 };

        for idx in (start_idx..self.columns().len()).rev() {
            let mut builder = SchemaBuilder::new();

            for column in self.columns()[..idx].iter().chain(self.columns()[idx+1..].iter()).cloned() {
                builder.add_column_by_ref(column);
            }

            let mut primary_key_columns = Vec::new();
            for pk_idx in 0..self.num_primary_key_columns() {
                if idx == pk_idx { continue; }
                primary_key_columns.push(self.columns()[pk_idx].name().to_owned());
            }
            builder.set_primary_key_by_ref(primary_key_columns);
            schemas.push(builder.build().unwrap());
        }

        Box::new(schemas.into_iter())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SchemaBuilder {
    columns: Vec<Column>,
    primary_key: Vec<String>,
    range_partition_columns: Vec<String>,
}

impl SchemaBuilder {

    pub fn new() -> SchemaBuilder {
        SchemaBuilder {
            columns: Vec::new(),
            primary_key: Vec::new(),
            range_partition_columns: Vec::new(),
        }
    }

    pub fn add_column(mut self, column: Column) -> SchemaBuilder {
        self.add_column_by_ref(column);
        self
    }

    pub fn add_column_by_ref(&mut self, column: Column) -> &mut SchemaBuilder {
        self.columns.push(column);
        self
    }

    pub fn set_primary_key<S>(mut self, columns: Vec<S>) -> SchemaBuilder where S: Into<String> {
        self.set_primary_key_by_ref(columns);
        self
    }

    pub fn set_primary_key_by_ref<S>(&mut self, columns: Vec<S>) -> &mut SchemaBuilder where S: Into<String> {
        self.primary_key = columns.into_iter().map(Into::into).collect();
        self
    }

    pub fn build(mut self) -> Result<Schema> {
        if self.primary_key.is_empty() {
            return Err(Error::InvalidArgument(
                    "primary key must have at least one column".to_owned()));
        }

        let mut columns = Vec::with_capacity(self.columns.len());

        for column_name in &self.primary_key {
            let idx = self.columns.iter().position(|col| col.name() == column_name);
            if let Some(idx) = idx {
                columns.push(self.columns.remove(idx));
            } else {
                return Err(Error::InvalidArgument(
                    format!("primary key column '{}' has no corresponding column",
                            column_name)))
            }
        }

        columns.extend(self.columns.drain(..));

        Ok(Schema::new(columns, self.primary_key.len()))
    }
}

#[cfg(test)]
pub mod tests {

    use super::*;
    use DataType;

    pub fn simple_schema() -> Schema {
        SchemaBuilder::new()
            .add_column(Column::builder("key", DataType::String).set_not_null())
            .add_column(Column::builder("val", DataType::String).set_not_null())
            .set_primary_key(vec!["key"])
            .build()
            .unwrap()
    }

    pub fn all_types_schema() -> Schema {
        SchemaBuilder::new()
            .add_column(Column::builder("key", DataType::Int32).set_not_null())
            .add_column(Column::builder("bool", DataType::Bool).set_not_null())
            .add_column(Column::builder("i8", DataType::Int8).set_not_null())
            .add_column(Column::builder("i16", DataType::Int16).set_not_null())
            .add_column(Column::builder("i32", DataType::Int32).set_not_null())
            .add_column(Column::builder("i64", DataType::Int64).set_not_null())
            .add_column(Column::builder("timestamp", DataType::Timestamp).set_not_null())
            .add_column(Column::builder("f32", DataType::Float).set_not_null())
            .add_column(Column::builder("f64", DataType::Double).set_not_null())
            .add_column(Column::builder("binary", DataType::Binary).set_not_null())
            .add_column(Column::builder("string", DataType::String).set_not_null())
            .add_column(Column::builder("nullable_bool", DataType::Bool).set_nullable())
            .add_column(Column::builder("nullable_i8", DataType::Int8).set_nullable())
            .add_column(Column::builder("nullable_i16", DataType::Int16).set_nullable())
            .add_column(Column::builder("nullable_i32", DataType::Int32).set_nullable())
            .add_column(Column::builder("nullable_i64", DataType::Int64).set_nullable())
            .add_column(Column::builder("nullable_timestamp", DataType::Timestamp).set_nullable())
            .add_column(Column::builder("nullable_f32", DataType::Float).set_nullable())
            .add_column(Column::builder("nullable_f64", DataType::Double).set_nullable())
            .add_column(Column::builder("nullable_binary", DataType::Binary).set_nullable())
            .add_column(Column::builder("nullable_string", DataType::String).set_nullable())
            .set_primary_key(vec!["key"])
            .build()
            .unwrap()
    }

    #[test]
    fn test_create_schema() {
        all_types_schema();
    }
}
