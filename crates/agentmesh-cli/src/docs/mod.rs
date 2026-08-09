//! Embedded AgentMesh docs catalog and CLI JSON contracts.

mod list;
mod show;

include!(concat!(env!("OUT_DIR"), "/docs_registry.rs"));

pub use list::docs_list_command;
pub use show::docs_show_command;
