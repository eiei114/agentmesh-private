//! Embedded AgentMesh docs catalog and CLI JSON contracts.

mod list;

include!(concat!(env!("OUT_DIR"), "/docs_registry.rs"));

pub use list::docs_list_command;
