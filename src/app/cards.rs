//! Project cards: a project led by its supervisor is drawn as one card that
//! shows the supervisor and sums up its workers, until `→` expands it into
//! every worker's own rows. Which projects are expanded is remembered across
//! restarts, and the list's selection only stops on the rows that are shown.

use std::{collections::BTreeSet, fs, ops::Range, path::PathBuf};

use crate::{model::DashboardAgent, paths::corgi_state_dir};

use super::App;

/// The projects whose cards are expanded, by the heading the dashboard groups
/// them under. Every other card is collapsed, which is the default.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CardMemory {
    /// `None` keeps the choice in memory only, which tests use.
    path: Option<PathBuf>,
    expanded: BTreeSet<String>,
}

impl CardMemory {
    /// Reads the expanded projects from `expanded-projects` in Corgi's state
    /// directory.
    pub(crate) fn load() -> Self {
        match corgi_state_dir() {
            Some(dir) => Self::load_from(dir.join("expanded-projects")),
            None => Self::default(),
        }
    }

    /// Reads the file at `path`, one project per line. A missing or
    /// unreadable file leaves every card collapsed, never an error.
    pub(crate) fn load_from(path: PathBuf) -> Self {
        let expanded = fs::read_to_string(&path)
            .map(|text| {
                text.lines()
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        Self {
            path: Some(path),
            expanded,
        }
    }

    pub(crate) fn is_expanded(&self, project: &str) -> bool {
        self.expanded.contains(project)
    }

    /// Expands or collapses `project`'s card, writing the choice down when
    /// it changed.
    pub(crate) fn set_expanded(&mut self, project: &str, expanded: bool) {
        let changed = if expanded {
            self.expanded.insert(project.to_string())
        } else {
            self.expanded.remove(project)
        };
        if changed {
            self.save();
        }
    }

    fn save(&self) {
        let Some(path) = &self.path else { return };
        if let Some(directory) = path.parent() {
            let _ = fs::create_dir_all(directory);
        }
        let mut text = String::new();
        for project in &self.expanded {
            text.push_str(project);
            text.push('\n');
        }
        let _ = fs::write(path, text);
    }
}

/// The runs of agents that share a project heading, in list order. The list
/// is sorted by project, so each project is one run.
pub(crate) fn project_runs(agents: &[DashboardAgent]) -> Vec<Range<usize>> {
    let mut runs: Vec<Range<usize>> = Vec::new();
    for (index, agent) in agents.iter().enumerate() {
        match runs.last_mut() {
            Some(run) if agents[run.start].project_group == agent.project_group => {
                run.end = index + 1;
            }
            _ => runs.push(index..index + 1),
        }
    }
    runs
}

/// Whether a project's run is drawn as a card: it is led by its supervisor, and
/// it is not the scratch sessions, which keep their own rows.
pub(crate) fn is_card(agents: &[DashboardAgent], run: &Range<usize>) -> bool {
    agents
        .get(run.start)
        .is_some_and(|lead| lead.supervisor && !lead.scratch)
}

impl App {
    /// Whether the card of the project `run` is expanded into its workers'
    /// rows.
    pub(crate) fn card_expanded(&self, run: &Range<usize>) -> bool {
        self.agents
            .get(run.start)
            .is_some_and(|lead| self.cards.is_expanded(&lead.project_group))
    }

    /// The run of the card agent `index` is drawn in, if it is in one.
    fn card_of(&self, index: usize) -> Option<Range<usize>> {
        project_runs(&self.agents)
            .into_iter()
            .find(|run| run.contains(&index))
            .filter(|run| is_card(&self.agents, run))
    }

    /// The agents the selection can stop on, in list order: every row that
    /// is drawn. A collapsed card is one stop, its corgi.
    pub(crate) fn selection_stops(&self) -> Vec<usize> {
        let mut stops = Vec::with_capacity(self.agents.len());
        for run in project_runs(&self.agents) {
            if is_card(&self.agents, &run) && !self.card_expanded(&run) {
                stops.push(run.start);
            } else {
                stops.extend(run);
            }
        }
        stops
    }

    /// Where the selection is among [`Self::selection_stops`]: an agent
    /// folded into a collapsed card counts as that card.
    pub(super) fn selected_stop(&self) -> usize {
        let stops = self.selection_stops();
        stops
            .iter()
            .position(|&stop| stop == self.selected)
            .or_else(|| {
                let card = self.card_of(self.selected)?;
                stops.iter().position(|&stop| stop == card.start)
            })
            .unwrap_or(0)
    }

    /// Selects the stop at `position`, or the last one when the list got
    /// shorter, so the selection stays on the same row of the list.
    pub(super) fn select_stop(&mut self, position: usize) {
        let stops = self.selection_stops();
        if let Some(&stop) = stops.get(position).or(stops.last()) {
            self.selected = stop;
        }
    }

    /// Moves the selection `delta` rows, around the ends.
    pub(super) fn move_selection(&mut self, delta: isize) {
        let stops = self.selection_stops();
        if stops.is_empty() {
            return;
        }
        let len = stops.len() as isize;
        let position = (self.selected_stop() as isize + delta).rem_euclid(len) as usize;
        self.select(stops[position]);
    }

    /// `→`: expands the selected project's card into its workers' rows.
    pub(super) fn expand_card(&mut self) {
        let Some(run) = self.card_of(self.selected) else {
            return;
        };
        let project = self.agents[run.start].project_group.clone();
        if !self.cards.is_expanded(&project) {
            self.cards.set_expanded(&project, true);
            self.status = format!("Expanded {project}");
        }
    }

    /// `←`: collapses the selected project's card back into its summary, and
    /// moves the selection from a worker row to the corgi.
    pub(super) fn collapse_card(&mut self) {
        let Some(run) = self.card_of(self.selected) else {
            return;
        };
        let project = self.agents[run.start].project_group.clone();
        if self.cards.is_expanded(&project) {
            self.cards.set_expanded(&project, false);
            self.status = format!("Collapsed {project}");
        }
        if self.selected != run.start {
            self.select(run.start);
        }
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyModifiers};

    use crate::{
        model::{AgentInfo, AgentState},
        test_support::{ScratchDir, test_app},
    };

    use super::{super::test_helpers::press, *};

    fn agent(pane: &str, group: &str, supervisor: bool) -> DashboardAgent {
        DashboardAgent {
            info: AgentInfo {
                pane_id: pane.into(),
                state: AgentState::Working,
                ..AgentInfo::default()
            },
            project_group: group.into(),
            supervisor,
            ..DashboardAgent::default()
        }
    }

    /// A corgi project of three, a project without one, and a scratch
    /// session, in the dashboard's order.
    fn herd() -> Vec<DashboardAgent> {
        vec![
            agent("corgi", "corgi", true),
            agent("worker-a", "corgi", false),
            agent("worker-b", "corgi", false),
            agent("loner", "notes", false),
            DashboardAgent {
                scratch: true,
                ..agent("scratch", "Scratch", false)
            },
        ]
    }

    fn selected_pane(app: &App) -> &str {
        &app.agents[app.selected].info.pane_id
    }

    #[test]
    fn the_choice_survives_a_restart_and_a_bad_file_leaves_cards_collapsed() {
        let dir = ScratchDir::new("cards");
        let path = dir.join("state/expanded-projects");

        // Missing: every card is collapsed.
        let mut memory = CardMemory::load_from(path.clone());
        assert!(!memory.is_expanded("corgi"));

        memory.set_expanded("corgi", true);
        memory.set_expanded("webshop", true);
        memory.set_expanded("webshop", false);
        let reloaded = CardMemory::load_from(path.clone());
        assert!(reloaded.is_expanded("corgi"));
        assert!(!reloaded.is_expanded("webshop"));

        // A file that is not text at all is no choice.
        fs::write(&path, [0xff, 0xfe, 0x00, 0x9f]).expect("write garbage");
        assert!(!CardMemory::load_from(path.clone()).is_expanded("corgi"));
        // Nor is a directory where the file should be.
        fs::remove_file(&path).expect("remove garbage");
        fs::create_dir_all(&path).expect("make a directory");
        assert!(!CardMemory::load_from(path).is_expanded("corgi"));
    }

    #[test]
    fn a_collapsed_card_is_one_stop_and_other_rows_are_their_own() {
        let mut app = test_app();
        app.agents = herd();

        assert_eq!(app.selection_stops(), [0, 3, 4]);
        press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE);
        assert_eq!(selected_pane(&app), "loner");
        press(&mut app, KeyCode::Char('k'), KeyModifiers::NONE);
        press(&mut app, KeyCode::Char('k'), KeyModifiers::NONE);
        assert_eq!(selected_pane(&app), "scratch");

        // Neither a project without a supervisor nor a scratch session has a
        // card to expand.
        press(&mut app, KeyCode::Right, KeyModifiers::NONE);
        assert!(!app.cards.is_expanded("Scratch"));
        app.selected = 3;
        press(&mut app, KeyCode::Right, KeyModifiers::NONE);
        assert!(!app.cards.is_expanded("notes"));
        assert_eq!(app.selection_stops(), [0, 3, 4]);
    }

    #[test]
    fn right_expands_the_card_into_its_workers_and_left_folds_them_back() {
        let mut app = test_app();
        app.agents = herd();

        press(&mut app, KeyCode::Right, KeyModifiers::NONE);
        assert!(app.cards.is_expanded("corgi"));
        assert_eq!(app.selected, 0);
        assert_eq!(app.selection_stops(), [0, 1, 2, 3, 4]);
        press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE);
        press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE);
        assert_eq!(selected_pane(&app), "worker-b");

        // From a worker row, the card folds and the supervisor is selected.
        press(&mut app, KeyCode::Left, KeyModifiers::NONE);
        assert!(!app.cards.is_expanded("corgi"));
        assert_eq!(selected_pane(&app), "corgi");
        assert_eq!(app.selection_stops(), [0, 3, 4]);
        // Folding a folded card does nothing.
        press(&mut app, KeyCode::Left, KeyModifiers::NONE);
        assert_eq!(selected_pane(&app), "corgi");
    }

    #[test]
    fn a_refresh_keeps_the_selection_on_the_same_row_of_the_list() {
        let mut app = test_app();
        app.agents = herd();
        app.selected = 3;
        let row = app.selected_stop();

        // A new worker in the collapsed card adds no row above the selection,
        // so the same project stays selected even though its index moved.
        app.agents.insert(1, agent("worker-new", "corgi", false));
        app.select_stop(row);
        assert_eq!(selected_pane(&app), "loner");

        // A worker selected when its card is folded some other way counts as
        // the card.
        app.selected = 2;
        assert_eq!(app.selected_stop(), 0);
    }
}
