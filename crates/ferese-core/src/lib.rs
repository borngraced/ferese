pub mod desktop;
pub mod notifications;
mod output;
mod workspace;

pub use output::{OutputError, OutputGeometry, OutputId, OutputWorkspaceMap, WorkspaceSwitch};
pub use workspace::{
    LayoutMode, WindowPlacement, Workspace, WorkspaceError, WorkspaceId, WorkspaceLayout,
    WorkspaceSet,
};
