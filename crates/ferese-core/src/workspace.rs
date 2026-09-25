use std::collections::{HashMap, HashSet};
use std::error::Error;
use std::fmt;

use ferese_layout::{
    Axis, ColumnWidth, Direction, GapConfig, LayoutError, LayoutResult, LayoutTree, Rect,
    ScrollingLayout, SizeConstraints, ViewportFocusStrategy, WindowId,
};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct WorkspaceId(pub u64);

#[derive(Debug)]
pub struct Workspace {
    pub id: WorkspaceId,
    pub name: String,
    pub layout: WorkspaceLayout,
    pub floating: Vec<WindowId>,
    pub last_focused: Option<WindowId>,
    pub fullscreen: Option<WindowId>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum LayoutMode {
    #[default]
    Scrolling,
    Tree,
}

#[derive(Debug)]
pub enum WorkspaceLayout {
    Scrolling(ScrollingLayout),
    Tree(LayoutTree),
}

impl Default for WorkspaceLayout {
    fn default() -> Self {
        Self::Scrolling(ScrollingLayout::default())
    }
}

impl WorkspaceLayout {
    pub fn new(
        mode: LayoutMode,
        default_column_width: ColumnWidth,
        focus_strategy: ViewportFocusStrategy,
    ) -> Self {
        match mode {
            LayoutMode::Scrolling => {
                let mut layout = ScrollingLayout::with_default_width(default_column_width);
                layout.set_focus_strategy(focus_strategy);
                Self::Scrolling(layout)
            }
            LayoutMode::Tree => Self::Tree(LayoutTree::default()),
        }
    }

    pub fn mode(&self) -> LayoutMode {
        match self {
            Self::Scrolling(_) => LayoutMode::Scrolling,
            Self::Tree(_) => LayoutMode::Tree,
        }
    }

    pub fn viewport_x(&self) -> Option<f64> {
        match self {
            Self::Scrolling(layout) => Some(layout.viewport_x()),
            Self::Tree(_) => None,
        }
    }

    pub fn preferred_window(&self) -> Option<WindowId> {
        match self {
            Self::Scrolling(layout) => layout.active_window(),
            Self::Tree(layout) => layout.window_ids().next(),
        }
    }

    pub fn contains(&self, window: WindowId) -> bool {
        match self {
            Self::Scrolling(layout) => layout.contains(window),
            Self::Tree(layout) => layout.contains(window),
        }
    }

    pub fn window_ids(&self) -> Box<dyn Iterator<Item = WindowId> + '_> {
        match self {
            Self::Scrolling(layout) => Box::new(layout.window_ids()),
            Self::Tree(layout) => Box::new(layout.window_ids()),
        }
    }

    pub fn insert(
        &mut self,
        window: WindowId,
        focused: Option<WindowId>,
        axis: Axis,
        ratio: f64,
    ) -> Result<(), LayoutError> {
        match self {
            Self::Scrolling(layout) => layout.insert(window, focused),
            Self::Tree(layout) => layout.insert(window, focused, axis, ratio).map(drop),
        }
    }

    pub fn remove(&mut self, window: WindowId) -> Result<(), LayoutError> {
        match self {
            Self::Scrolling(layout) => layout.remove(window),
            Self::Tree(layout) => layout.remove(window),
        }
    }

    pub fn stack_window(&mut self, window: WindowId, target: WindowId) -> Result<(), LayoutError> {
        match self {
            Self::Scrolling(layout) => layout.move_into_column(window, target),
            Self::Tree(layout) => layout.stack_window(window, target),
        }
    }

    pub fn activate_window(&mut self, window: WindowId) -> Result<(), LayoutError> {
        match self {
            Self::Scrolling(layout) => layout.focus(window),
            Self::Tree(layout) => layout.activate_window(window),
        }
    }

    pub fn geometry(&self, bounds: Rect) -> Result<HashMap<WindowId, Rect>, LayoutError> {
        match self {
            Self::Scrolling(layout) => {
                let mut layout = layout.clone();
                layout
                    .geometry_with_constraints(bounds, GapConfig::default(), &HashMap::new(), None)
                    .map(|result| result.geometry)
            }
            Self::Tree(layout) => layout.geometry(bounds),
        }
    }

    pub fn geometry_with_constraints(
        &mut self,
        bounds: Rect,
        gaps: GapConfig,
        constraints: &HashMap<WindowId, SizeConstraints>,
        focused: Option<WindowId>,
    ) -> Result<LayoutResult, LayoutError> {
        match self {
            Self::Scrolling(layout) => layout.geometry_with_constraints(
                bounds,
                gaps,
                constraints,
                focused.filter(|window| layout.contains(*window)),
            ),
            Self::Tree(layout) => {
                layout.geometry_with_constraints(bounds, gaps, constraints, focused)
            }
        }
    }

    pub fn automatic_axis(
        &self,
        focused: Option<WindowId>,
        bounds: Rect,
    ) -> Result<Axis, LayoutError> {
        match self {
            Self::Scrolling(_) => Ok(Axis::Horizontal),
            Self::Tree(layout) => layout.automatic_axis(focused, bounds),
        }
    }

    pub fn directional_neighbor(
        &self,
        window: WindowId,
        direction: Direction,
        bounds: Rect,
    ) -> Result<Option<WindowId>, LayoutError> {
        match self {
            Self::Scrolling(layout) => layout.directional_neighbor(window, direction),
            Self::Tree(layout) => layout.directional_neighbor(window, direction, bounds),
        }
    }

    pub fn move_window(
        &mut self,
        window: WindowId,
        direction: Direction,
        bounds: Rect,
    ) -> Result<bool, LayoutError> {
        match self {
            Self::Scrolling(layout) => layout.move_window(window, direction),
            Self::Tree(layout) => layout.move_window(window, direction, bounds),
        }
    }

    pub fn resize_window(
        &mut self,
        window: WindowId,
        direction: Direction,
        amount: f64,
    ) -> Result<bool, LayoutError> {
        match self {
            Self::Scrolling(layout) => layout.resize_window(window, direction, amount),
            Self::Tree(layout) => layout.resize_window(window, direction, amount),
        }
    }

    pub fn extract_window(&mut self, window: WindowId) -> Result<bool, LayoutError> {
        match self {
            Self::Scrolling(layout) => {
                let grouped = layout
                    .columns()
                    .iter()
                    .find(|column| column.windows.contains(&window))
                    .is_some_and(|column| column.windows.len() > 1);
                layout.extract_to_column(window)?;
                Ok(grouped)
            }
            Self::Tree(_) => Ok(false),
        }
    }

    pub fn cycle_column_width(
        &mut self,
        window: WindowId,
        presets: &[ColumnWidth],
    ) -> Result<bool, LayoutError> {
        match self {
            Self::Scrolling(layout) => layout.cycle_column_width(window, presets),
            Self::Tree(_) => Ok(false),
        }
    }

    pub fn center_window(
        &mut self,
        window: WindowId,
        bounds: Rect,
        gaps: GapConfig,
        constraints: &HashMap<WindowId, SizeConstraints>,
    ) -> Result<bool, LayoutError> {
        match self {
            Self::Scrolling(layout) => layout.center_window(window, bounds, gaps, constraints),
            Self::Tree(_) => Ok(false),
        }
    }

    pub fn validate(&self) -> Result<(), LayoutError> {
        match self {
            Self::Scrolling(layout) => layout.validate(),
            Self::Tree(layout) => layout.validate(),
        }
    }

    pub fn set_mode(
        &mut self,
        mode: LayoutMode,
        bounds: Rect,
        focused: Option<WindowId>,
        default_column_width: ColumnWidth,
        focus_strategy: ViewportFocusStrategy,
    ) -> Result<bool, LayoutError> {
        if self.mode() == mode {
            return Ok(false);
        }

        let replacement = match self {
            Self::Scrolling(layout) => {
                let columns = layout.columns().to_vec();
                let mut tree = LayoutTree::default();
                let mut previous = None;

                for column in columns {
                    for (row, window) in column.windows.into_iter().enumerate() {
                        let axis = if row == 0 {
                            Axis::Horizontal
                        } else {
                            Axis::Vertical
                        };
                        tree.insert(window, previous, axis, 0.5)?;
                        previous = Some(window);
                    }
                }
                if let Some(focused) = focused.filter(|window| tree.contains(*window)) {
                    tree.activate_window(focused)?;
                }
                Self::Tree(tree)
            }
            Self::Tree(layout) => {
                let windows = layout.window_ids_in_reading_order(bounds)?;
                let mut scrolling = ScrollingLayout::with_default_width(default_column_width);
                scrolling.set_focus_strategy(focus_strategy);
                let mut previous = None;

                for window in windows {
                    scrolling.insert(window, previous)?;
                    previous = Some(window);
                }
                if let Some(focused) = focused.filter(|window| scrolling.contains(*window)) {
                    scrolling.focus(focused)?;
                }
                Self::Scrolling(scrolling)
            }
        };

        *self = replacement;
        debug_assert!(self.validate().is_ok());
        Ok(true)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WindowPlacement {
    Tiled,
    Floating { rect: Rect },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorkspaceError {
    InvalidNumericName(u32),
    UnknownWorkspace(WorkspaceId),
    DuplicateWindow(WindowId),
    InvalidState(&'static str),
    Layout(LayoutError),
}

impl fmt::Display for WorkspaceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidNumericName(index) => {
                write!(formatter, "invalid workspace number {index}")
            }
            Self::UnknownWorkspace(workspace) => {
                write!(formatter, "unknown workspace {workspace:?}")
            }
            Self::DuplicateWindow(window) => write!(formatter, "duplicate window {window:?}"),
            Self::InvalidState(reason) => write!(formatter, "invalid workspace state: {reason}"),
            Self::Layout(error) => error.fmt(formatter),
        }
    }
}

impl Error for WorkspaceError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Layout(error) => Some(error),
            _ => None,
        }
    }
}

impl From<LayoutError> for WorkspaceError {
    fn from(error: LayoutError) -> Self {
        Self::Layout(error)
    }
}

#[derive(Debug)]
pub struct WorkspaceSet {
    active: WorkspaceId,
    workspaces: HashMap<WorkspaceId, Workspace>,
    names: HashMap<String, WorkspaceId>,
    window_workspaces: HashMap<WindowId, WorkspaceId>,
    placements: HashMap<WindowId, WindowPlacement>,
    default_layout_mode: LayoutMode,
    default_column_width: ColumnWidth,
    scrolling_focus_strategy: ViewportFocusStrategy,
    next_id: u64,
}

impl Default for WorkspaceSet {
    fn default() -> Self {
        Self::new(
            LayoutMode::default(),
            ColumnWidth::default(),
            ViewportFocusStrategy::default(),
        )
    }
}

impl WorkspaceSet {
    pub fn new(
        default_layout_mode: LayoutMode,
        default_column_width: ColumnWidth,
        scrolling_focus_strategy: ViewportFocusStrategy,
    ) -> Self {
        let active = WorkspaceId(1);
        let workspace = Workspace {
            id: active,
            name: "1".to_owned(),
            layout: WorkspaceLayout::new(
                default_layout_mode,
                default_column_width,
                scrolling_focus_strategy,
            ),
            floating: Vec::new(),
            last_focused: None,
            fullscreen: None,
        };
        let workspaces = HashMap::from([(active, workspace)]);
        let names = HashMap::from([("1".to_owned(), active)]);

        Self {
            active,
            workspaces,
            names,
            window_workspaces: HashMap::new(),
            placements: HashMap::new(),
            default_layout_mode,
            default_column_width,
            scrolling_focus_strategy,
            next_id: 2,
        }
    }

    pub fn active_id(&self) -> WorkspaceId {
        self.active
    }

    pub fn active(&self) -> &Workspace {
        self.workspaces
            .get(&self.active)
            .expect("active workspace always exists")
    }

    pub fn active_mut(&mut self) -> &mut Workspace {
        self.workspaces
            .get_mut(&self.active)
            .expect("active workspace always exists")
    }

    pub fn activate(&mut self, workspace: WorkspaceId) -> Result<Option<WindowId>, WorkspaceError> {
        if !self.workspaces.contains_key(&workspace) {
            return Err(WorkspaceError::UnknownWorkspace(workspace));
        }

        self.active = workspace;

        debug_assert!(self.validate().is_ok());
        Ok(self.active().fullscreen.or(self.active().last_focused))
    }

    pub fn workspace(&self, id: WorkspaceId) -> Option<&Workspace> {
        self.workspaces.get(&id)
    }

    pub fn workspace_mut(&mut self, id: WorkspaceId) -> Option<&mut Workspace> {
        self.workspaces.get_mut(&id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Workspace> {
        self.workspaces.values()
    }

    pub fn workspace_for_window(&self, window: WindowId) -> Option<WorkspaceId> {
        self.window_workspaces.get(&window).copied()
    }

    pub fn placement(&self, window: WindowId) -> Option<WindowPlacement> {
        self.placements.get(&window).copied()
    }

    pub fn set_active_layout_mode(
        &mut self,
        mode: LayoutMode,
        bounds: Rect,
    ) -> Result<bool, WorkspaceError> {
        let default_column_width = self.default_column_width;
        let focus_strategy = self.scrolling_focus_strategy;
        let workspace = self.active_mut();
        let focused = tiled_focus(workspace);
        let changed = workspace.layout.set_mode(
            mode,
            bounds,
            focused,
            default_column_width,
            focus_strategy,
        )?;

        debug_assert!(self.validate().is_ok());
        Ok(changed)
    }

    pub fn set_floating_rect(
        &mut self,
        window: WindowId,
        rect: Rect,
    ) -> Result<(), WorkspaceError> {
        let placement = self
            .placements
            .get_mut(&window)
            .ok_or(LayoutError::UnknownWindow(window))?;

        if !matches!(placement, WindowPlacement::Floating { .. }) {
            return Err(WorkspaceError::InvalidState("window is not floating"));
        }

        *placement = WindowPlacement::Floating {
            rect: normalized_floating_rect(rect),
        };

        debug_assert!(self.validate().is_ok());
        Ok(())
    }

    pub fn focus_window(&mut self, window: WindowId) -> Result<(), WorkspaceError> {
        if self.workspace_for_window(window) != Some(self.active) {
            return Err(WorkspaceError::InvalidState(
                "focused window is not on the active workspace",
            ));
        }
        if self
            .active()
            .fullscreen
            .is_some_and(|fullscreen| fullscreen != window)
        {
            return Err(WorkspaceError::InvalidState(
                "focused window is hidden by fullscreen",
            ));
        }

        if self.placement(window) == Some(WindowPlacement::Tiled) {
            self.active_mut().layout.activate_window(window)?;
        }
        self.active_mut().last_focused = Some(window);

        debug_assert!(self.validate().is_ok());
        Ok(())
    }

    pub fn stack_window(
        &mut self,
        window: WindowId,
        target: WindowId,
    ) -> Result<(), WorkspaceError> {
        let workspace = self
            .workspace_for_window(window)
            .ok_or(LayoutError::UnknownWindow(window))?;

        if self.workspace_for_window(target) != Some(workspace) {
            return Err(WorkspaceError::InvalidState(
                "stack windows belong to different workspaces",
            ));
        }
        if self.placement(window) != Some(WindowPlacement::Tiled)
            || self.placement(target) != Some(WindowPlacement::Tiled)
        {
            return Err(WorkspaceError::InvalidState("stack window is not tiled"));
        }

        let workspace = self
            .workspaces
            .get_mut(&workspace)
            .ok_or(WorkspaceError::InvalidState("window workspace is missing"))?;
        workspace.layout.stack_window(window, target)?;
        workspace.last_focused = Some(window);

        debug_assert!(self.validate().is_ok());
        Ok(())
    }

    pub fn extract_window(&mut self, window: WindowId) -> Result<bool, WorkspaceError> {
        if self.workspace_for_window(window) != Some(self.active)
            || self.placement(window) != Some(WindowPlacement::Tiled)
        {
            return Err(WorkspaceError::InvalidState(
                "extracted window is not tiled on the active workspace",
            ));
        }

        let changed = self.active_mut().layout.extract_window(window)?;
        debug_assert!(self.validate().is_ok());
        Ok(changed)
    }

    pub fn cycle_column_width(
        &mut self,
        window: WindowId,
        presets: &[ColumnWidth],
    ) -> Result<bool, WorkspaceError> {
        if self.workspace_for_window(window) != Some(self.active)
            || self.placement(window) != Some(WindowPlacement::Tiled)
        {
            return Err(WorkspaceError::InvalidState(
                "resized window is not tiled on the active workspace",
            ));
        }

        let changed = self
            .active_mut()
            .layout
            .cycle_column_width(window, presets)?;
        debug_assert!(self.validate().is_ok());
        Ok(changed)
    }

    pub fn center_window(
        &mut self,
        window: WindowId,
        bounds: Rect,
        gaps: GapConfig,
        constraints: &HashMap<WindowId, SizeConstraints>,
    ) -> Result<bool, WorkspaceError> {
        if self.workspace_for_window(window) != Some(self.active)
            || self.placement(window) != Some(WindowPlacement::Tiled)
        {
            return Err(WorkspaceError::InvalidState(
                "centered window is not tiled on the active workspace",
            ));
        }

        let changed = self
            .active_mut()
            .layout
            .center_window(window, bounds, gaps, constraints)?;
        debug_assert!(self.validate().is_ok());
        Ok(changed)
    }

    pub fn ensure_numeric(&mut self, index: u32) -> Result<WorkspaceId, WorkspaceError> {
        if index == 0 {
            return Err(WorkspaceError::InvalidNumericName(index));
        }

        let name = index.to_string();

        if let Some(id) = self.names.get(&name) {
            return Ok(*id);
        }

        let id = WorkspaceId(self.next_id);
        self.next_id += 1;
        self.names.insert(name.clone(), id);
        self.workspaces.insert(
            id,
            Workspace {
                id,
                name,
                layout: WorkspaceLayout::new(
                    self.default_layout_mode,
                    self.default_column_width,
                    self.scrolling_focus_strategy,
                ),
                floating: Vec::new(),
                last_focused: None,
                fullscreen: None,
            },
        );

        debug_assert!(self.validate().is_ok());
        Ok(id)
    }

    pub fn switch_to_numeric(&mut self, index: u32) -> Result<Option<WindowId>, WorkspaceError> {
        let workspace = self.ensure_numeric(index)?;
        self.activate(workspace)
    }

    pub fn insert_window(
        &mut self,
        window: WindowId,
        axis: Axis,
        ratio: f64,
    ) -> Result<(), WorkspaceError> {
        if self.window_workspaces.contains_key(&window) {
            return Err(WorkspaceError::DuplicateWindow(window));
        }

        let workspace = self.active_mut();
        let focused = tiled_focus(workspace);
        workspace.layout.insert(window, focused, axis, ratio)?;
        if workspace.fullscreen.is_none() {
            workspace.last_focused = Some(window);
        }
        self.window_workspaces.insert(window, self.active);
        self.placements.insert(window, WindowPlacement::Tiled);

        debug_assert!(self.validate().is_ok());
        Ok(())
    }

    pub fn insert_floating_window(
        &mut self,
        window: WindowId,
        workspace_id: WorkspaceId,
        rect: Rect,
        focus: bool,
    ) -> Result<(), WorkspaceError> {
        if self.window_workspaces.contains_key(&window) {
            return Err(WorkspaceError::DuplicateWindow(window));
        }

        let workspace = self
            .workspaces
            .get_mut(&workspace_id)
            .ok_or(WorkspaceError::UnknownWorkspace(workspace_id))?;
        workspace.floating.push(window);
        if focus && workspace.fullscreen.is_none() {
            workspace.last_focused = Some(window);
        }
        self.window_workspaces.insert(window, workspace_id);
        self.placements.insert(
            window,
            WindowPlacement::Floating {
                rect: normalized_floating_rect(rect),
            },
        );

        debug_assert!(self.validate().is_ok());
        Ok(())
    }

    pub fn remove_window(&mut self, window: WindowId) -> Result<(), WorkspaceError> {
        let workspace_id = self
            .window_workspaces
            .remove(&window)
            .ok_or(LayoutError::UnknownWindow(window))?;
        let workspace = self
            .workspaces
            .get_mut(&workspace_id)
            .ok_or(WorkspaceError::UnknownWorkspace(workspace_id))?;

        match self.placements.remove(&window) {
            Some(WindowPlacement::Tiled) => workspace.layout.remove(window)?,
            Some(WindowPlacement::Floating { .. }) => {
                workspace.floating.retain(|candidate| *candidate != window);
            }
            None => return Err(LayoutError::UnknownWindow(window).into()),
        }

        if workspace.fullscreen == Some(window) {
            workspace.fullscreen = None;
        }

        if workspace.last_focused == Some(window) {
            workspace.last_focused = first_window(workspace);
        }

        debug_assert!(self.validate().is_ok());
        Ok(())
    }

    pub fn move_window_to_numeric(
        &mut self,
        window: WindowId,
        index: u32,
        axis: Axis,
        ratio: f64,
    ) -> Result<WorkspaceId, WorkspaceError> {
        let source_id = self
            .workspace_for_window(window)
            .ok_or(LayoutError::UnknownWindow(window))?;
        let destination_id = self.ensure_numeric(index)?;

        if source_id == destination_id {
            return Ok(destination_id);
        }

        let placement = self
            .placements
            .get(&window)
            .copied()
            .ok_or(LayoutError::UnknownWindow(window))?;
        let source = self
            .workspaces
            .get_mut(&source_id)
            .ok_or(WorkspaceError::UnknownWorkspace(source_id))?;

        match placement {
            WindowPlacement::Tiled => source.layout.remove(window)?,
            WindowPlacement::Floating { .. } => {
                source.floating.retain(|candidate| *candidate != window);
            }
        }

        let was_fullscreen = source.fullscreen == Some(window);
        if was_fullscreen {
            source.fullscreen = None;
        }

        if source.last_focused == Some(window) {
            source.last_focused = first_window(source);
        }

        let destination = self
            .workspaces
            .get_mut(&destination_id)
            .ok_or(WorkspaceError::UnknownWorkspace(destination_id))?;
        match placement {
            WindowPlacement::Tiled => {
                let focused = tiled_focus(destination);
                destination.layout.insert(window, focused, axis, ratio)?;
            }
            WindowPlacement::Floating { .. } => destination.floating.push(window),
        }
        if was_fullscreen {
            destination.fullscreen = Some(window);
        }
        if destination.fullscreen.is_none() || was_fullscreen {
            destination.last_focused = Some(window);
        }
        self.window_workspaces.insert(window, destination_id);

        debug_assert!(self.validate().is_ok());
        Ok(destination_id)
    }

    pub fn toggle_floating(
        &mut self,
        window: WindowId,
        floating_rect: Rect,
        axis: Axis,
        ratio: f64,
    ) -> Result<WindowPlacement, WorkspaceError> {
        let workspace_id = self
            .workspace_for_window(window)
            .ok_or(LayoutError::UnknownWindow(window))?;
        let workspace = self
            .workspaces
            .get_mut(&workspace_id)
            .ok_or(WorkspaceError::UnknownWorkspace(workspace_id))?;
        let placement = self
            .placements
            .get_mut(&window)
            .ok_or(LayoutError::UnknownWindow(window))?;

        *placement = match *placement {
            WindowPlacement::Tiled => {
                workspace.layout.remove(window)?;
                workspace.floating.push(window);
                WindowPlacement::Floating {
                    rect: normalized_floating_rect(floating_rect),
                }
            }
            WindowPlacement::Floating { .. } => {
                workspace.floating.retain(|candidate| *candidate != window);
                let focused = tiled_focus(workspace);
                workspace.layout.insert(window, focused, axis, ratio)?;
                WindowPlacement::Tiled
            }
        };

        let result = *placement;

        debug_assert!(self.validate().is_ok());
        Ok(result)
    }

    pub fn toggle_fullscreen(&mut self, window: WindowId) -> Result<bool, WorkspaceError> {
        let enabled = self
            .workspace_for_window(window)
            .and_then(|workspace| self.workspaces.get(&workspace))
            .is_none_or(|workspace| workspace.fullscreen != Some(window));

        self.set_fullscreen(window, enabled)?;
        Ok(enabled)
    }

    pub fn set_fullscreen(
        &mut self,
        window: WindowId,
        enabled: bool,
    ) -> Result<bool, WorkspaceError> {
        let workspace_id = self
            .workspace_for_window(window)
            .ok_or(LayoutError::UnknownWindow(window))?;
        let workspace = self
            .workspaces
            .get_mut(&workspace_id)
            .ok_or(WorkspaceError::UnknownWorkspace(workspace_id))?;

        let changed = if enabled {
            let changed = workspace.fullscreen != Some(window);
            workspace.fullscreen = Some(window);
            workspace.last_focused = Some(window);
            changed
        } else if workspace.fullscreen == Some(window) {
            workspace.fullscreen = None;
            true
        } else {
            false
        };

        debug_assert!(self.validate().is_ok());
        Ok(changed)
    }

    pub fn validate(&self) -> Result<(), WorkspaceError> {
        if !self.workspaces.contains_key(&self.active) {
            return Err(WorkspaceError::InvalidState("active workspace is missing"));
        }
        if self.names.len() != self.workspaces.len() {
            return Err(WorkspaceError::InvalidState(
                "workspace name index is inconsistent",
            ));
        }

        let mut seen_windows = HashSet::new();

        for (id, workspace) in &self.workspaces {
            if workspace.id != *id || self.names.get(&workspace.name) != Some(id) {
                return Err(WorkspaceError::InvalidState(
                    "workspace index is inconsistent",
                ));
            }

            workspace.layout.validate()?;

            for window in workspace.layout.window_ids() {
                if !seen_windows.insert(window)
                    || self.window_workspaces.get(&window) != Some(id)
                    || self.placements.get(&window) != Some(&WindowPlacement::Tiled)
                {
                    return Err(WorkspaceError::InvalidState(
                        "window ownership is inconsistent",
                    ));
                }
            }

            let mut seen_floating = HashSet::new();

            for window in &workspace.floating {
                if !seen_floating.insert(*window)
                    || !seen_windows.insert(*window)
                    || self.window_workspaces.get(window) != Some(id)
                    || !matches!(
                        self.placements.get(window),
                        Some(WindowPlacement::Floating { .. })
                    )
                {
                    return Err(WorkspaceError::InvalidState(
                        "floating window ownership is inconsistent",
                    ));
                }
            }

            if workspace
                .last_focused
                .is_some_and(|window| self.window_workspaces.get(&window) != Some(id))
            {
                return Err(WorkspaceError::InvalidState(
                    "workspace focus is inconsistent",
                ));
            }

            if workspace
                .fullscreen
                .is_some_and(|window| self.window_workspaces.get(&window) != Some(id))
            {
                return Err(WorkspaceError::InvalidState(
                    "fullscreen window ownership is inconsistent",
                ));
            }
        }

        if seen_windows.len() != self.window_workspaces.len()
            || self.placements.len() != self.window_workspaces.len()
        {
            return Err(WorkspaceError::InvalidState(
                "window index retains stale entries",
            ));
        }

        Ok(())
    }
}

fn first_window(workspace: &Workspace) -> Option<WindowId> {
    workspace.layout.preferred_window().or_else(|| {
        workspace
            .floating
            .iter()
            .copied()
            .min_by_key(|window| window.0)
    })
}

fn tiled_focus(workspace: &Workspace) -> Option<WindowId> {
    workspace
        .last_focused
        .filter(|window| workspace.layout.contains(*window))
        .or_else(|| workspace.layout.window_ids().min_by_key(|window| window.0))
}

fn normalized_floating_rect(rect: Rect) -> Rect {
    Rect::new(
        finite_or_zero(rect.x),
        finite_or_zero(rect.y),
        finite_or_zero(rect.width).max(1.0),
        finite_or_zero(rect.height).max(1.0),
    )
}

fn finite_or_zero(value: f64) -> f64 {
    if value.is_finite() { value } else { 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numeric_workspaces_are_created_lazily_with_stable_ids() {
        let mut workspaces = WorkspaceSet::default();
        let first = workspaces.active_id();
        let ninth = workspaces.ensure_numeric(9).unwrap();

        assert_eq!(workspaces.ensure_numeric(1), Ok(first));
        assert_eq!(workspaces.ensure_numeric(9), Ok(ninth));
        assert_ne!(first, ninth);
        assert!(workspaces.validate().is_ok());
    }

    #[test]
    fn configured_layout_defaults_apply_to_new_workspaces() {
        let mut workspaces = WorkspaceSet::new(
            LayoutMode::Scrolling,
            ColumnWidth::Full,
            ViewportFocusStrategy::Minimal,
        );
        workspaces
            .insert_window(WindowId(1), Axis::Horizontal, 0.5)
            .unwrap();
        let first = workspaces
            .active_mut()
            .layout
            .geometry_with_constraints(
                Rect::new(0.0, 0.0, 1_000.0, 800.0),
                GapConfig::default(),
                &HashMap::new(),
                Some(WindowId(1)),
            )
            .unwrap();

        assert_eq!(
            first.geometry[&WindowId(1)],
            Rect::new(10.0, 10.0, 980.0, 780.0)
        );
        workspaces.switch_to_numeric(2).unwrap();
        assert_eq!(workspaces.active().layout.mode(), LayoutMode::Scrolling);
    }

    #[test]
    fn switching_restores_workspace_focus() {
        let mut workspaces = WorkspaceSet::default();
        workspaces
            .insert_window(WindowId(1), Axis::Horizontal, 0.5)
            .unwrap();

        assert_eq!(workspaces.switch_to_numeric(2).unwrap(), None);
        workspaces
            .insert_window(WindowId(2), Axis::Horizontal, 0.5)
            .unwrap();

        assert_eq!(workspaces.switch_to_numeric(1).unwrap(), Some(WindowId(1)));
        assert_eq!(workspaces.switch_to_numeric(2).unwrap(), Some(WindowId(2)));
        assert!(workspaces.validate().is_ok());
    }

    #[test]
    fn moving_a_window_updates_membership_without_switching() {
        let mut workspaces = WorkspaceSet::default();
        workspaces
            .insert_window(WindowId(1), Axis::Horizontal, 0.5)
            .unwrap();
        workspaces
            .insert_window(WindowId(2), Axis::Horizontal, 0.5)
            .unwrap();
        let first = workspaces.active_id();
        let second = workspaces
            .move_window_to_numeric(WindowId(2), 2, Axis::Horizontal, 0.5)
            .unwrap();

        assert_eq!(workspaces.active_id(), first);
        assert_eq!(workspaces.workspace_for_window(WindowId(2)), Some(second));
        assert!(
            !workspaces
                .workspace(first)
                .unwrap()
                .layout
                .contains(WindowId(2))
        );
        assert!(
            workspaces
                .workspace(second)
                .unwrap()
                .layout
                .contains(WindowId(2))
        );
        assert!(workspaces.validate().is_ok());
    }

    #[test]
    fn removing_focus_chooses_a_deterministic_remaining_window() {
        let mut workspaces = WorkspaceSet::default();
        workspaces
            .insert_window(WindowId(2), Axis::Horizontal, 0.5)
            .unwrap();
        workspaces
            .insert_window(WindowId(1), Axis::Horizontal, 0.5)
            .unwrap();
        workspaces.remove_window(WindowId(1)).unwrap();

        assert_eq!(workspaces.active().last_focused, Some(WindowId(2)));
        assert!(workspaces.validate().is_ok());
    }

    #[test]
    fn closing_a_scrolling_column_focuses_its_adjacent_column() {
        let mut workspaces = WorkspaceSet::default();
        for id in 1..=4 {
            workspaces
                .insert_window(WindowId(id), Axis::Horizontal, 0.5)
                .unwrap();
        }
        workspaces.focus_window(WindowId(3)).unwrap();

        workspaces.remove_window(WindowId(3)).unwrap();

        assert_eq!(workspaces.active().last_focused, Some(WindowId(4)));
    }

    #[test]
    fn focus_rejects_a_window_on_an_inactive_workspace() {
        let mut workspaces = WorkspaceSet::default();
        workspaces
            .insert_window(WindowId(1), Axis::Horizontal, 0.5)
            .unwrap();
        workspaces
            .move_window_to_numeric(WindowId(1), 2, Axis::Horizontal, 0.5)
            .unwrap();

        assert_eq!(
            workspaces.focus_window(WindowId(1)),
            Err(WorkspaceError::InvalidState(
                "focused window is not on the active workspace"
            ))
        );
        assert!(workspaces.validate().is_ok());
    }

    #[test]
    fn floating_toggle_removes_and_restores_tiled_membership() {
        let mut workspaces = WorkspaceSet::default();
        workspaces
            .insert_window(WindowId(1), Axis::Horizontal, 0.5)
            .unwrap();
        let rect = Rect::new(10.0, 20.0, 640.0, 480.0);

        assert_eq!(
            workspaces
                .toggle_floating(WindowId(1), rect, Axis::Horizontal, 0.5)
                .unwrap(),
            WindowPlacement::Floating { rect }
        );
        assert!(!workspaces.active().layout.contains(WindowId(1)));
        assert_eq!(workspaces.active().floating, vec![WindowId(1)]);

        assert_eq!(
            workspaces
                .toggle_floating(WindowId(1), rect, Axis::Horizontal, 0.5)
                .unwrap(),
            WindowPlacement::Tiled
        );
        assert!(workspaces.active().layout.contains(WindowId(1)));
        assert!(workspaces.active().floating.is_empty());
        assert!(workspaces.validate().is_ok());
    }

    #[test]
    fn floating_geometry_updates_are_authoritative_and_validated() {
        let mut workspaces = WorkspaceSet::default();
        workspaces
            .insert_window(WindowId(1), Axis::Horizontal, 0.5)
            .unwrap();
        assert_eq!(
            workspaces.set_floating_rect(WindowId(1), Rect::default()),
            Err(WorkspaceError::InvalidState("window is not floating"))
        );

        workspaces
            .toggle_floating(
                WindowId(1),
                Rect::new(10.0, 20.0, 640.0, 480.0),
                Axis::Horizontal,
                0.5,
            )
            .unwrap();
        workspaces
            .set_floating_rect(WindowId(1), Rect::new(30.0, 40.0, f64::NAN, -2.0))
            .unwrap();

        assert_eq!(
            workspaces.placement(WindowId(1)),
            Some(WindowPlacement::Floating {
                rect: Rect::new(30.0, 40.0, 1.0, 1.0)
            })
        );
        assert!(workspaces.validate().is_ok());
    }

    #[test]
    fn floating_window_can_follow_its_parent_workspace_without_focus() {
        let mut workspaces = WorkspaceSet::default();
        workspaces
            .insert_window(WindowId(1), Axis::Horizontal, 0.5)
            .unwrap();
        let parent_workspace = workspaces.active_id();
        workspaces.switch_to_numeric(2).unwrap();
        workspaces
            .insert_floating_window(
                WindowId(2),
                parent_workspace,
                Rect::new(20.0, 30.0, 400.0, 300.0),
                false,
            )
            .unwrap();

        assert_eq!(
            workspaces.workspace_for_window(WindowId(2)),
            Some(parent_workspace)
        );
        assert_eq!(
            workspaces.workspace(parent_workspace).unwrap().last_focused,
            Some(WindowId(1))
        );
        assert!(workspaces.validate().is_ok());
    }

    #[test]
    fn fullscreen_preserves_underlying_placement() {
        let mut workspaces = WorkspaceSet::default();
        workspaces
            .insert_window(WindowId(1), Axis::Horizontal, 0.5)
            .unwrap();
        let before = workspaces.placement(WindowId(1));

        assert!(workspaces.toggle_fullscreen(WindowId(1)).unwrap());
        assert_eq!(workspaces.active().fullscreen, Some(WindowId(1)));
        assert_eq!(workspaces.placement(WindowId(1)), before);
        assert!(!workspaces.set_fullscreen(WindowId(1), true).unwrap());

        assert!(!workspaces.toggle_fullscreen(WindowId(1)).unwrap());
        assert_eq!(workspaces.active().fullscreen, None);
        assert_eq!(workspaces.placement(WindowId(1)), before);
        assert!(!workspaces.set_fullscreen(WindowId(1), false).unwrap());
        assert!(workspaces.validate().is_ok());
    }

    #[test]
    fn mapping_under_fullscreen_does_not_change_focus() {
        let mut workspaces = WorkspaceSet::default();
        workspaces
            .insert_window(WindowId(1), Axis::Horizontal, 0.5)
            .unwrap();
        workspaces.toggle_fullscreen(WindowId(1)).unwrap();
        workspaces
            .insert_window(WindowId(2), Axis::Horizontal, 0.5)
            .unwrap();

        assert_eq!(workspaces.active().last_focused, Some(WindowId(1)));
        assert_eq!(workspaces.active().fullscreen, Some(WindowId(1)));
        assert!(workspaces.active().layout.contains(WindowId(2)));
        assert!(workspaces.validate().is_ok());
    }

    #[test]
    fn switching_to_fullscreen_restores_the_visible_window() {
        let mut workspaces = WorkspaceSet::default();
        workspaces
            .insert_window(WindowId(1), Axis::Horizontal, 0.5)
            .unwrap();
        workspaces.toggle_fullscreen(WindowId(1)).unwrap();
        workspaces.switch_to_numeric(2).unwrap();
        workspaces
            .insert_window(WindowId(2), Axis::Horizontal, 0.5)
            .unwrap();
        workspaces
            .move_window_to_numeric(WindowId(2), 1, Axis::Horizontal, 0.5)
            .unwrap();

        assert_eq!(workspaces.switch_to_numeric(1).unwrap(), Some(WindowId(1)));
        assert_eq!(workspaces.active().last_focused, Some(WindowId(1)));
        assert_eq!(
            workspaces.focus_window(WindowId(2)),
            Err(WorkspaceError::InvalidState(
                "focused window is hidden by fullscreen"
            ))
        );
        assert!(workspaces.validate().is_ok());
    }

    #[test]
    fn grouping_tiled_windows_preserves_workspace_membership() {
        let mut workspaces = WorkspaceSet::default();
        workspaces
            .insert_window(WindowId(1), Axis::Horizontal, 0.5)
            .unwrap();
        workspaces
            .insert_window(WindowId(2), Axis::Horizontal, 0.5)
            .unwrap();
        workspaces.stack_window(WindowId(2), WindowId(1)).unwrap();

        assert_eq!(workspaces.active().last_focused, Some(WindowId(2)));
        assert_eq!(workspaces.active().layout.mode(), LayoutMode::Scrolling);
        assert_eq!(
            workspaces
                .active()
                .layout
                .geometry(Rect::new(0.0, 0.0, 100.0, 80.0))
                .unwrap()
                .len(),
            2
        );
        assert!(workspaces.validate().is_ok());
    }

    #[test]
    fn layout_mode_conversion_preserves_membership_and_focus() {
        let mut workspaces = WorkspaceSet::default();
        for id in 1..=4 {
            workspaces
                .insert_window(WindowId(id), Axis::Horizontal, 0.5)
                .unwrap();
        }
        workspaces.focus_window(WindowId(2)).unwrap();
        let bounds = Rect::new(0.0, 0.0, 1_000.0, 800.0);

        assert!(
            workspaces
                .set_active_layout_mode(LayoutMode::Tree, bounds)
                .unwrap()
        );
        assert_eq!(workspaces.active().layout.mode(), LayoutMode::Tree);
        assert_eq!(workspaces.active().last_focused, Some(WindowId(2)));
        assert_eq!(
            workspaces
                .active()
                .layout
                .window_ids()
                .collect::<HashSet<_>>(),
            HashSet::from([WindowId(1), WindowId(2), WindowId(3), WindowId(4)])
        );

        assert!(
            workspaces
                .set_active_layout_mode(LayoutMode::Scrolling, bounds)
                .unwrap()
        );
        assert_eq!(workspaces.active().layout.mode(), LayoutMode::Scrolling);
        assert_eq!(workspaces.active().last_focused, Some(WindowId(2)));
        assert!(workspaces.validate().is_ok());
    }

    #[test]
    fn scrolling_column_commands_preserve_workspace_state() {
        let mut workspaces = WorkspaceSet::default();
        for id in 1..=3 {
            workspaces
                .insert_window(WindowId(id), Axis::Horizontal, 0.5)
                .unwrap();
        }

        workspaces.stack_window(WindowId(3), WindowId(2)).unwrap();
        assert!(workspaces.extract_window(WindowId(3)).unwrap());
        assert!(
            workspaces
                .cycle_column_width(
                    WindowId(3),
                    &[ColumnWidth::Proportion(0.5), ColumnWidth::Full],
                )
                .unwrap()
        );
        let bounds = Rect::new(0.0, 0.0, 1_000.0, 800.0);
        let _ = workspaces
            .center_window(WindowId(3), bounds, GapConfig::default(), &HashMap::new())
            .unwrap();

        assert_eq!(workspaces.active().last_focused, Some(WindowId(3)));
        assert!(workspaces.validate().is_ok());
    }

    #[test]
    fn randomized_workspace_sequences_preserve_all_invariants() {
        let mut workspaces = WorkspaceSet::default();
        let mut windows = Vec::new();
        let mut next_window = 1;
        let mut random = 0xc0de_cafe_u64;

        for _ in 0..2_000 {
            random = random
                .wrapping_mul(2_862_933_555_777_941_757)
                .wrapping_add(3_037_000_493);

            match random % 7 {
                0 if windows.len() < 48 => {
                    let window = WindowId(next_window);
                    next_window += 1;
                    workspaces
                        .insert_window(window, random_axis(random), 0.5)
                        .unwrap();
                    windows.push(window);
                }
                1 if !windows.is_empty() => {
                    let index = random as usize % windows.len();
                    let window = windows.swap_remove(index);
                    workspaces.remove_window(window).unwrap();
                }
                2 => {
                    workspaces
                        .switch_to_numeric((random.rotate_left(11) % 4 + 1) as u32)
                        .unwrap();
                }
                3 if !windows.is_empty() => {
                    let window = windows[random as usize % windows.len()];
                    workspaces
                        .move_window_to_numeric(
                            window,
                            (random.rotate_left(17) % 4 + 1) as u32,
                            random_axis(random),
                            0.5,
                        )
                        .unwrap();
                }
                4 if !windows.is_empty() => {
                    let window = windows[random as usize % windows.len()];
                    workspaces
                        .toggle_floating(window, random_rect(random), random_axis(random), 0.5)
                        .unwrap();
                }
                5 if !windows.is_empty() => {
                    let window = windows[random as usize % windows.len()];
                    workspaces.toggle_fullscreen(window).unwrap();
                }
                6 if !windows.is_empty() => {
                    let candidates = windows
                        .iter()
                        .copied()
                        .filter(|window| {
                            workspaces.workspace_for_window(*window) == Some(workspaces.active_id())
                                && workspaces
                                    .active()
                                    .fullscreen
                                    .is_none_or(|fullscreen| fullscreen == *window)
                        })
                        .collect::<Vec<_>>();

                    if let Some(window) = candidates.get(random as usize % candidates.len().max(1))
                    {
                        workspaces.focus_window(*window).unwrap();
                    }
                }
                _ => {}
            }

            assert!(workspaces.validate().is_ok());
        }
    }

    fn random_axis(random: u64) -> Axis {
        if random & 1 == 0 {
            Axis::Horizontal
        } else {
            Axis::Vertical
        }
    }

    fn random_rect(random: u64) -> Rect {
        Rect::new(
            (random % 400) as f64,
            (random.rotate_left(9) % 300) as f64,
            (random.rotate_left(21) % 900 + 1) as f64,
            (random.rotate_left(33) % 700 + 1) as f64,
        )
    }
}
