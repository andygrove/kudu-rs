use std::net::SocketAddr;
use std::time::Instant;

use kudu_pb::master::{
    AlterTableRequestPB, AlterTableResponsePB,
    CreateTableRequestPB, CreateTableResponsePB,
    DeleteTableRequestPB, DeleteTableResponsePB,
    GetMasterRegistrationRequestPB, GetMasterRegistrationResponsePB,
    GetTableLocationsRequestPB, GetTableLocationsResponsePB,
    GetTableSchemaRequestPB, GetTableSchemaResponsePB,
    GetTabletLocationsRequestPB, GetTabletLocationsResponsePB,
    IsAlterTableDoneRequestPB, IsAlterTableDoneResponsePB,
    IsCreateTableDoneRequestPB, IsCreateTableDoneResponsePB,
    ListMastersRequestPB, ListMastersResponsePB,
    ListTablesRequestPB, ListTablesResponsePB,
    ListTabletServersRequestPB, ListTabletServersResponsePB,
    PingRequestPB, PingResponsePB,
};
use rpc::Rpc;

const SERVICE_NAME: &'static str = "kudu.master.MasterService";

// When macros in type position and concat_idents! land the 3rd and 4th param can be dropped.
// If/when Rust gets a snake -> camel ident converter the 2nd param can be dropped.
macro_rules! rpc {
    ($fn_name:ident, $rpc_name:ident, $request_type:ident, $response_type:ident) => {
        pub fn $fn_name(addr: SocketAddr, deadline: Instant, request: $request_type) -> Rpc {
            Rpc {
                addr: addr,
                service_name: SERVICE_NAME,
                method_name: stringify!($rpc_name),
                deadline: deadline,
                required_feature_flags: Vec::new(),
                request: Box::new(request),
                response: Box::new($response_type::new()),
                sidecars: Vec::new(),
                callback: None,
                cancel: None,
                fail_fast: true,
            }
        }
    };
}

rpc!(ping, Ping, PingRequestPB, PingResponsePB);
rpc!(get_tablet_locations, GetTabletLocations, GetTabletLocationsRequestPB, GetTabletLocationsResponsePB);
rpc!(create_table, CreateTable, CreateTableRequestPB, CreateTableResponsePB);
rpc!(is_create_table_done, IsCreateTableDone, IsCreateTableDoneRequestPB, IsCreateTableDoneResponsePB);
rpc!(delete_table, DeleteTable, DeleteTableRequestPB, DeleteTableResponsePB);
rpc!(alter_table, AlterTable, AlterTableRequestPB, AlterTableResponsePB);
rpc!(is_alter_table_done, IsAlterTableDone, IsAlterTableDoneRequestPB, IsAlterTableDoneResponsePB);
rpc!(list_tables, ListTables, ListTablesRequestPB, ListTablesResponsePB);
rpc!(get_table_locations, GetTableLocations, GetTableLocationsRequestPB, GetTableLocationsResponsePB);
rpc!(get_table_schema, GetTableSchema, GetTableSchemaRequestPB, GetTableSchemaResponsePB);
rpc!(list_tablet_servers, ListTabletServers, ListTabletServersRequestPB, ListTabletServersResponsePB);
rpc!(list_masters, ListMasters, ListMastersRequestPB, ListMastersResponsePB);
rpc!(get_master_registration, GetMasterRegistration, GetMasterRegistrationRequestPB, GetMasterRegistrationResponsePB);
