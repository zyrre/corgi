//! The machines the window shows Corgi on: this one, always first, and the
//! ones the user added, each reached as `ssh HOST corgi serve`. The list is
//! kept in `$STATE/gui-machines.json` with the one last shown; the status
//! bar lists them, and its "Add machine" form (`MachineForm`) adds and edits
//! them, offering the hosts of `~/.ssh/config`.

use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use corgi_core::protocol::Choice;
use serde::{Deserialize, Serialize};

use crate::{
    form::{Key, Picker},
    link::{Backoff, Link},
    options::Options,
    store::{Closing, Outbox, Store},
};

/// What the bar calls this machine.
pub const THIS_MACHINE: &str = "this machine";
/// The `corgi` a machine runs when nothing else is given: the one on the
/// host's `PATH`.
pub const DEFAULT_CORGI: &str = "corgi";

/// A machine the user added.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedMachine {
    /// Shown in the status bar.
    pub name: String,
    /// What `ssh` is given: a `Host` of `~/.ssh/config`, `user@host`, …
    pub host: String,
    /// The path to `corgi` on the host.
    #[serde(default = "default_corgi")]
    pub corgi: String,
}

fn default_corgi() -> String {
    DEFAULT_CORGI.into()
}

impl SavedMachine {
    /// How the window runs `corgi serve` there.
    pub fn options(&self) -> Options {
        Options {
            ssh: Some(self.host.clone()),
            session: None,
            socket: None,
            corgi: (self.corgi != DEFAULT_CORGI).then(|| self.corgi.clone()),
        }
    }
}

/// `$STATE/gui-machines.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedMachines {
    /// The host of the machine shown when the window last switched; `None`
    /// for this machine.
    pub active: Option<String>,
    pub machines: Vec<SavedMachine>,
}

impl SavedMachines {
    pub fn path() -> Option<PathBuf> {
        corgi_core::paths::corgi_state_dir().map(|dir| dir.join("gui-machines.json"))
    }

    /// The list kept at `path`; none when there is no file or it cannot be
    /// read. A machine without a host, or with one listed before, is
    /// dropped.
    pub fn load(path: Option<&Path>) -> Self {
        let mut saved = path
            .and_then(|path| fs::read_to_string(path).ok())
            .and_then(|text| serde_json::from_str::<SavedMachines>(&text).ok())
            .unwrap_or_default();
        let mut hosts = Vec::new();
        saved.machines.retain(|machine| {
            let keep = !machine.host.trim().is_empty() && !hosts.contains(&machine.host);
            hosts.push(machine.host.clone());
            keep
        });
        for machine in &mut saved.machines {
            if machine.name.trim().is_empty() {
                machine.name = machine.host.clone();
            }
            if machine.corgi.trim().is_empty() {
                machine.corgi = DEFAULT_CORGI.into();
            }
        }
        if saved
            .active
            .as_ref()
            .is_some_and(|active| !saved.machines.iter().any(|m| &m.host == active))
        {
            saved.active = None;
        }
        saved
    }

    /// Keeps the list at `path`, replacing it whole through a rename.
    pub fn save(&self, path: Option<&Path>) -> Result<(), String> {
        let Some(path) = path else { return Ok(()) };
        let text = serde_json::to_string_pretty(self).map_err(|error| error.to_string())?;
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(|error| error.to_string())?;
        }
        let temporary = path.with_extension("json.tmp");
        fs::write(&temporary, text)
            .and_then(|()| fs::rename(&temporary, path))
            .map_err(|error| format!("could not save {}: {error}", path.display()))
    }
}

// ---- A machine while the window runs ----

/// One machine as the window keeps it: its connection to `corgi serve`, the
/// state that sent, and what the user made of its overview (the selection,
/// the expanded row, the collapsed cards), kept while another is shown.
pub struct Machine {
    pub name: String,
    pub options: Options,
    /// As `gui-machines.json` keeps it; `None` for this machine and for
    /// `--ssh`'s one-off. A saved machine `--ssh` names runs with the
    /// command line's options, which are not kept.
    pub saved: Option<SavedMachine>,
    pub link: Option<Arc<Link>>,
    /// The number of its latest connection: events of an older one, or of
    /// another machine's, are not its.
    pub link_number: u64,
    pub backoff: Backoff,
    pub store: Store,
    pub outbox: Outbox,
    /// The agent the keys act on: none at start, until a key or a click
    /// picks one.
    pub selected: Option<String>,
    /// A row has been picked since the window opened: from then on, when
    /// the selected row goes, another is selected in its place.
    pub picked: bool,
    /// The selected row is expanded into its session's latest turns.
    pub expanded: bool,
    /// The session `corgi serve` was asked to read the transcript of.
    pub watching: Option<String>,
    /// The project groups whose cards show only their supervisor.
    pub collapsed: HashSet<String>,
    /// The card groups this connection has shown: a card not among them
    /// starts collapsed, and one the user expanded stays so.
    pub seen: HashSet<String>,
    /// The questions closed with ×, until the ledger drops them.
    pub closing: Closing,
    /// Pause all or Resume all running here while another machine shows:
    /// the header's button says so again when this one is back.
    pub all_running: Option<&'static str>,
}

impl Machine {
    pub fn new(name: impl Into<String>, options: Options, saved: Option<SavedMachine>) -> Self {
        Self {
            name: name.into(),
            options,
            saved,
            link: None,
            link_number: 0,
            backoff: Backoff::default(),
            store: Store::default(),
            outbox: Outbox::default(),
            selected: None,
            picked: false,
            expanded: false,
            watching: None,
            collapsed: HashSet::new(),
            seen: HashSet::new(),
            closing: Closing::default(),
            all_running: None,
        }
    }

    pub fn saved(machine: &SavedMachine) -> Self {
        Self::new(
            machine.name.clone(),
            machine.options(),
            Some(machine.clone()),
        )
    }

    /// The host `ssh` reaches it by; `None` for this machine.
    pub fn host(&self) -> Option<&str> {
        self.options.ssh.as_deref()
    }

    /// What the status bar calls it: its name, and the Herdr session it
    /// reads when one was asked for.
    pub fn label(&self) -> String {
        format!("{}{}", self.name, self.options.herdr())
    }
}

/// Moves the window from machine `*active` to machine `to`, which is
/// shown from then on with what it kept: its state, selection, collapsed
/// cards and expanded row are its own. The header's running Pause all or
/// Resume all, the window's while a machine shows, is parked with the
/// machine left and comes back with it. Returns the machine left, `None`
/// when there is no switch.
pub fn switch(
    machines: &mut [Machine],
    active: &mut usize,
    all_running: &mut Option<&'static str>,
    to: usize,
) -> Option<usize> {
    if to == *active || to >= machines.len() {
        return None;
    }
    let left = *active;
    machines[left].all_running = all_running.take();
    *active = to;
    *all_running = machines[to].all_running.take();
    Some(left)
}

/// Removes machine `index`, not the one shown, keeping `*active` on the
/// same machine.
pub fn remove(machines: &mut Vec<Machine>, active: &mut usize, index: usize) {
    if index == *active || index >= machines.len() {
        return;
    }
    machines.remove(index);
    if *active > index {
        *active -= 1;
    }
}

/// The machines of a run: this one first, with the command line's
/// `--session`, `--socket` and `--corgi` unless `--ssh` was given, then the
/// saved ones; `--ssh HOST` shows HOST, a saved machine of that host or a
/// one-off that is not saved. Returns them and the one to show.
pub fn machines_of_run(options: &Options, saved: &SavedMachines) -> (Vec<Machine>, usize) {
    let local = match options.ssh {
        Some(_) => Options {
            ssh: None,
            session: None,
            socket: None,
            corgi: None,
        },
        None => options.clone(),
    };
    let mut machines = vec![Machine::new(THIS_MACHINE, local, None)];
    machines.extend(saved.machines.iter().map(Machine::saved));
    let shown = |host: &str| machines.iter().position(|m| m.host() == Some(host));
    let active = match &options.ssh {
        Some(host) => match shown(host) {
            Some(at) => {
                // The command line's options hold for this run.
                let machine = &mut machines[at];
                machine.options.session = options.session.clone();
                machine.options.socket = options.socket.clone();
                if options.corgi.is_some() {
                    machine.options.corgi = options.corgi.clone();
                }
                at
            }
            None => {
                machines.push(Machine::new(host.clone(), options.clone(), None));
                machines.len() - 1
            }
        },
        None => saved.active.as_deref().and_then(shown).unwrap_or_default(),
    };
    (machines, active)
}

// ---- ~/.ssh/config ----

/// The hosts `~/.ssh/config` names, in its order, for the form's list.
pub fn ssh_config_hosts() -> Vec<String> {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return Vec::new();
    };
    let ssh = home.join(".ssh");
    let mut hosts = Vec::new();
    read_ssh_config(&ssh.join("config"), &ssh, &home, 0, &mut hosts);
    hosts
}

/// Adds the `Host` names of the config at `path` to `hosts`, following its
/// `Include`s (relative to `~/.ssh`, `*` allowed in the file name). A
/// pattern (`*`, `?`, a negation) names no one host, so it is skipped.
fn read_ssh_config(path: &Path, ssh: &Path, home: &Path, depth: u8, hosts: &mut Vec<String>) {
    if depth > 8 {
        return;
    }
    let Ok(text) = fs::read_to_string(path) else {
        return;
    };
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        let (keyword, rest) = match line.split_once(|c: char| c.is_whitespace() || c == '=') {
            Some((keyword, rest)) => (keyword, rest.trim_start_matches(['=', ' ', '\t'])),
            None => continue,
        };
        let words = rest
            .split_whitespace()
            .map(|word| word.trim_matches('"'))
            .filter(|word| !word.is_empty());
        if keyword.eq_ignore_ascii_case("host") {
            for word in words {
                let pattern = word.contains(['*', '?', '!']);
                if !pattern && !hosts.iter().any(|host| host == word) {
                    hosts.push(word.to_string());
                }
            }
        } else if keyword.eq_ignore_ascii_case("include") {
            for word in words {
                for included in included_files(word, ssh, home) {
                    read_ssh_config(&included, ssh, home, depth + 1, hosts);
                }
            }
        }
    }
}

/// The files an `Include` names, sorted as `ssh` reads them.
fn included_files(word: &str, ssh: &Path, home: &Path) -> Vec<PathBuf> {
    let path = match word.strip_prefix("~/") {
        Some(rest) => home.join(rest),
        None if word.starts_with('/') => PathBuf::from(word),
        None => ssh.join(word),
    };
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    if !name.contains('*') {
        return vec![path];
    }
    let (Some(dir), Some((before, after))) = (path.parent(), name.split_once('*')) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            name.len() >= before.len() + after.len()
                && name.starts_with(before)
                && name.ends_with(after)
        })
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .collect();
    files.sort();
    files
}

// ---- The form ----

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MachineRow {
    Host,
    Corgi,
    Name,
}

impl MachineRow {
    pub const ALL: [MachineRow; 3] = [MachineRow::Host, MachineRow::Corgi, MachineRow::Name];

    pub fn label(self) -> &'static str {
        match self {
            MachineRow::Host => "SSH host",
            MachineRow::Corgi => "Corgi",
            MachineRow::Name => "Name",
        }
    }
}

/// What a key in the form asks of the window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormEffect {
    None,
    Submit,
    Cancel,
}

/// The "Add machine" form, and the same form to edit one. Every value is
/// preset: the host's list opens at once, `corgi` is `corgi` on the host's
/// `PATH`, and the name follows the host until typed. `↑`/`↓` and `Tab`
/// move, typing edits the row (on the host, filtering its list), `Enter`
/// saves and `Esc` cancels; a value refused keeps the form open on its row.
#[derive(Debug, Clone, PartialEq)]
pub struct MachineForm {
    /// The host of the machine edited; `None` adds one.
    pub editing: Option<String>,
    pub host: String,
    pub corgi: String,
    /// The name typed; empty follows the host.
    pub name: String,
    pub row: MachineRow,
    /// The host's list, open while the host row is edited.
    pub picker: Option<Picker>,
    /// `~/.ssh/config`'s hosts.
    pub hosts: Vec<String>,
    pub error: Option<String>,
}

impl MachineForm {
    pub fn add(hosts: Vec<String>) -> Self {
        Self {
            editing: None,
            host: String::new(),
            corgi: DEFAULT_CORGI.into(),
            name: String::new(),
            row: MachineRow::Host,
            picker: Some(Picker::default()),
            hosts,
            error: None,
        }
    }

    pub fn edit(machine: &SavedMachine, hosts: Vec<String>) -> Self {
        Self {
            editing: Some(machine.host.clone()),
            host: machine.host.clone(),
            corgi: machine.corgi.clone(),
            name: if machine.name == machine.host {
                String::new()
            } else {
                machine.name.clone()
            },
            row: MachineRow::Host,
            picker: None,
            hosts,
            error: None,
        }
    }

    pub fn title(&self) -> &'static str {
        if self.editing.is_some() {
            "Edit machine"
        } else {
            "Add machine"
        }
    }

    /// The name the machine gets: the one typed, else the host.
    pub fn name(&self) -> &str {
        if self.name.is_empty() {
            self.host.trim()
        } else {
            &self.name
        }
    }

    /// The row's value as the form shows it.
    pub fn shown(&self, row: MachineRow) -> &str {
        match row {
            MachineRow::Host => &self.host,
            MachineRow::Corgi => &self.corgi,
            MachineRow::Name => self.name(),
        }
    }

    /// What is said after the row's value.
    pub fn detail(&self, row: MachineRow) -> &'static str {
        match row {
            MachineRow::Host if self.host.is_empty() => "a Host of ~/.ssh/config, or user@host",
            MachineRow::Host if self.hosts.iter().any(|host| host == self.host.trim()) => {
                "from ~/.ssh/config"
            }
            MachineRow::Host => "",
            MachineRow::Corgi if self.corgi == DEFAULT_CORGI => "on the host's PATH",
            MachineRow::Corgi => "on the host",
            MachineRow::Name if self.name.is_empty() => "the host's, as the status bar shows it",
            MachineRow::Name => "as the status bar shows it",
        }
    }

    /// The host's open list: every host of `~/.ssh/config` holding what is
    /// typed, then what is typed as a host of its own.
    pub fn picker_rows(&self) -> Vec<Choice> {
        if self.picker.is_none() {
            return Vec::new();
        }
        let typed = self.host.trim();
        let lower = typed.to_lowercase();
        let mut rows: Vec<Choice> = self
            .hosts
            .iter()
            .filter(|host| host.to_lowercase().contains(&lower))
            .map(|host| Choice {
                value: host.clone(),
                label: host.clone(),
                detail: String::new(),
                badge: String::new(),
            })
            .collect();
        // The one typed exactly comes first.
        if let Some(at) = rows.iter().position(|row| row.value == typed) {
            let exact = rows.remove(at);
            rows.insert(0, exact);
        } else if !typed.is_empty() {
            rows.push(Choice {
                value: typed.into(),
                label: typed.into(),
                detail: String::new(),
                badge: "as typed".into(),
            });
        }
        rows
    }

    /// Typed text, or pasted: into the focused row, the host's list opening.
    pub fn type_text(&mut self, text: &str) {
        let text: String = text.chars().filter(|c| !c.is_control()).collect();
        if text.is_empty() {
            return;
        }
        self.error = None;
        match self.row {
            MachineRow::Host => {
                self.host.push_str(&text);
                self.picker = Some(Picker::default());
            }
            MachineRow::Corgi => self.corgi.push_str(&text),
            MachineRow::Name => {
                if self.name.is_empty() {
                    self.name = self.host.trim().to_string();
                }
                self.name.push_str(&text);
            }
        }
    }

    fn backspace(&mut self) {
        self.error = None;
        match self.row {
            MachineRow::Host => {
                self.host.pop();
                self.picker = Some(Picker::default());
            }
            MachineRow::Corgi => {
                self.corgi.pop();
            }
            MachineRow::Name => {
                if self.name.is_empty() {
                    self.name = self.host.trim().to_string();
                }
                self.name.pop();
            }
        }
    }

    /// Handles `key`.
    pub fn key(&mut self, key: Key) -> FormEffect {
        if self.picker.is_some()
            && let Some(effect) = self.picker_key(&key)
        {
            return effect;
        }
        match key {
            Key::Enter => FormEffect::Submit,
            Key::Escape => FormEffect::Cancel,
            Key::Down | Key::Tab => self.step(1, key == Key::Tab),
            Key::Up | Key::BackTab => self.step(-1, key == Key::BackTab),
            Key::Space if self.row == MachineRow::Host => {
                self.picker = Some(Picker::default());
                FormEffect::None
            }
            Key::Space => {
                self.type_text(" ");
                FormEffect::None
            }
            Key::Char(c) => {
                self.type_text(&c.to_string());
                FormEffect::None
            }
            Key::Backspace => {
                self.backspace();
                FormEffect::None
            }
            Key::Left | Key::Right => FormEffect::None,
        }
    }

    /// A key while the host's list is open; `None` leaves it to the form.
    /// Enter takes the highlighted host and saves, as every other row is
    /// preset; Tab takes it and moves on.
    fn picker_key(&mut self, key: &Key) -> Option<FormEffect> {
        let count = self.picker_rows().len();
        let picker = self.picker.as_mut()?;
        match key {
            Key::Escape => {
                self.picker = None;
                Some(FormEffect::None)
            }
            Key::Up | Key::Down => {
                if count > 0 {
                    let delta = if *key == Key::Up { -1 } else { 1 };
                    let cursor = picker.cursor.min(count - 1) as isize;
                    picker.cursor = (cursor + delta).rem_euclid(count as isize) as usize;
                }
                Some(FormEffect::None)
            }
            Key::Enter | Key::Tab => {
                let cursor = picker.cursor;
                if let Some(choice) = self.picker_rows().into_iter().nth(cursor) {
                    self.host = choice.value;
                }
                self.picker = None;
                None
            }
            Key::BackTab => {
                self.picker = None;
                None
            }
            Key::Char(_) | Key::Space | Key::Backspace | Key::Left | Key::Right => None,
        }
    }

    /// Moves `delta` rows; `Tab` wraps, the arrows stop at the ends.
    fn step(&mut self, delta: isize, wrap: bool) -> FormEffect {
        let rows = MachineRow::ALL;
        let at = rows.iter().position(|row| *row == self.row).unwrap_or(0) as isize;
        let count = rows.len() as isize;
        let next = if wrap {
            (at + delta).rem_euclid(count)
        } else {
            (at + delta).clamp(0, count - 1)
        };
        self.row = rows[next as usize];
        self.picker = None;
        FormEffect::None
    }

    /// The machine the form describes, checked against the `names` and
    /// `hosts` the other machines have. A refusal moves to the row at fault
    /// and keeps everything typed.
    pub fn machine(&mut self, names: &[String], hosts: &[String]) -> Option<SavedMachine> {
        let host = self.host.trim().to_string();
        let corgi = self.corgi.trim().to_string();
        let name = self.name().trim().to_string();
        let refusal = if host.is_empty() {
            Some((MachineRow::Host, "Give the SSH host".to_string()))
        } else if host.starts_with('-') || host.contains(char::is_whitespace) {
            Some((
                MachineRow::Host,
                "An SSH host is one word and does not start with -".to_string(),
            ))
        } else if hosts.contains(&host) {
            Some((MachineRow::Host, format!("{host} is in the list already")))
        } else if corgi.is_empty() {
            Some((
                MachineRow::Corgi,
                "Give the path to corgi on the host, or corgi for the one on its PATH".to_string(),
            ))
        } else if name.eq_ignore_ascii_case(THIS_MACHINE)
            || names.iter().any(|other| other.eq_ignore_ascii_case(&name))
        {
            Some((
                MachineRow::Name,
                format!("Another machine is called {name}"),
            ))
        } else {
            None
        };
        if let Some((row, error)) = refusal {
            self.row = row;
            self.picker = None;
            self.error = Some(error);
            return None;
        }
        Some(SavedMachine { name, host, corgi })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("corgi-gui-machines-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn machine(name: &str, host: &str) -> SavedMachine {
        SavedMachine {
            name: name.into(),
            host: host.into(),
            corgi: DEFAULT_CORGI.into(),
        }
    }

    #[test]
    fn the_list_is_kept_across_runs_and_a_bad_file_gives_none() {
        let dir = scratch("kept");
        let path = dir.join("gui-machines.json");
        assert_eq!(SavedMachines::load(Some(&path)), SavedMachines::default());
        let saved = SavedMachines {
            active: Some("omarchy".into()),
            machines: vec![
                machine("omarchy", "omarchy"),
                SavedMachine {
                    corgi: "~/.local/bin/corgi".into(),
                    ..machine("box", "peter@box")
                },
            ],
        };
        saved.save(Some(&path)).unwrap();
        assert_eq!(SavedMachines::load(Some(&path)), saved);
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains(r#""host": "peter@box""#), "{text}");

        fs::write(&path, "not json").unwrap();
        assert_eq!(SavedMachines::load(Some(&path)), SavedMachines::default());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_hand_edited_file_is_made_sane() {
        let dir = scratch("sane");
        let path = dir.join("gui-machines.json");
        fs::write(
            &path,
            r#"{"active":"gone","machines":[
                {"name":"","host":"a"},
                {"name":"again","host":"a"},
                {"name":"none","host":" "},
                {"name":"b","host":"b","corgi":""}
            ],"later":1}"#,
        )
        .unwrap();
        let saved = SavedMachines::load(Some(&path));
        assert_eq!(saved.active, None, "an active host no longer listed");
        assert_eq!(saved.machines, [machine("a", "a"), machine("b", "b")]);
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn each_machine_runs_serve_over_ssh_with_its_own_corgi() {
        let (program, args) = machine("omarchy", "omarchy").options().command();
        assert_eq!(program, "ssh");
        assert!(args.windows(2).any(|pair| pair == ["-o", "BatchMode=yes"]));
        assert_eq!(&args[args.len() - 2..], ["omarchy", "corgi serve"]);
        let elsewhere = SavedMachine {
            corgi: "~/bin/my corgi".into(),
            ..machine("box", "peter@box")
        };
        let (_, args) = elsewhere.options().command();
        assert_eq!(
            &args[args.len() - 2..],
            ["peter@box", "~/'bin/my corgi' serve"]
        );
        assert_eq!(elsewhere.options().place(), "peter@box");
    }

    #[test]
    fn the_ssh_config_s_hosts_are_listed_without_patterns_and_includes_followed() {
        let home = scratch("ssh");
        let ssh = home.join(".ssh");
        fs::create_dir_all(ssh.join("config.d")).unwrap();
        fs::write(
            ssh.join("config"),
            "# mine\nInclude config.d/*.conf\nHost omarchy box\n  HostName 10.0.0.2\nHost *.internal !x\nhost=devbox\nHost * \nMatch all\nHost box\n",
        )
        .unwrap();
        fs::write(ssh.join("config.d/work.conf"), "Host \"work-vm\"\n").unwrap();
        fs::write(ssh.join("config.d/notes.txt"), "Host ignored\n").unwrap();
        let mut hosts = Vec::new();
        read_ssh_config(&ssh.join("config"), &ssh, &home, 0, &mut hosts);
        assert_eq!(hosts, ["work-vm", "omarchy", "box", "devbox"]);
        let _ = fs::remove_dir_all(home);
    }

    fn options(args: &[&str]) -> Options {
        match Options::parse(args.iter().map(|arg| arg.to_string())) {
            Ok(crate::options::Parsed::Run(options)) => options,
            other => panic!("{other:?}"),
        }
    }

    fn two_saved(active: Option<&str>) -> SavedMachines {
        SavedMachines {
            active: active.map(String::from),
            machines: vec![machine("omarchy", "omarchy"), machine("box", "peter@box")],
        }
    }

    #[test]
    fn this_machine_comes_first_and_the_saved_active_one_shows() {
        let (machines, active) = machines_of_run(
            &options(&["--session", "work"]),
            &two_saved(Some("peter@box")),
        );
        let names: Vec<&str> = machines.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, [THIS_MACHINE, "omarchy", "box"]);
        assert_eq!(active, 2);
        assert_eq!(machines[0].label(), "this machine · session work");
        assert_eq!(
            machines[0].options.command().1,
            ["serve", "--session", "work"]
        );
        assert!(machines[0].saved.is_none());
        assert_eq!(machines[1].options.command().0, "ssh");
        let (_, active) = machines_of_run(&options(&[]), &two_saved(None));
        assert_eq!(active, 0);
    }

    #[test]
    fn ssh_on_the_command_line_shows_its_host_for_this_run_without_saving_it() {
        // A host not saved: a one-off, last, shown, with the command line's
        // session; this machine takes none of the remote's options.
        let run = options(&["--ssh", "devbox", "--session", "w", "--corgi", "/c"]);
        let (machines, active) = machines_of_run(&run, &two_saved(Some("omarchy")));
        assert_eq!(machines.len(), 4);
        assert_eq!(active, 3);
        assert_eq!(machines[3].name, "devbox");
        assert!(machines[3].saved.is_none());
        assert_eq!(machines[3].options, run);
        assert_eq!(machines[0].options.command().1, ["serve"]);
        // A saved host: that machine, shown with the command line's options
        // for this run while its saved record stays as it was.
        let run = options(&["--ssh", "omarchy", "--corgi", "/opt/corgi"]);
        let (machines, active) = machines_of_run(&run, &two_saved(None));
        assert_eq!((machines.len(), active), (3, 1));
        assert_eq!(machines[1].options.corgi.as_deref(), Some("/opt/corgi"));
        assert_eq!(machines[1].saved, Some(machine("omarchy", "omarchy")));
    }

    #[test]
    fn each_machine_keeps_its_own_view_across_a_switch_and_back() {
        use corgi_core::protocol::{Agent, ServerMessage, Snapshot, State};
        let snapshot = |ids: &[&str]| {
            ServerMessage::Snapshot(Box::new(Snapshot {
                seq: 1,
                now: 10,
                state: State {
                    agents: ids
                        .iter()
                        .map(|id| Agent {
                            pane_id: id.to_string(),
                            ..Agent::default()
                        })
                        .collect(),
                    ..State::default()
                },
            }))
        };
        let (mut machines, mut active) = machines_of_run(&options(&[]), &two_saved(None));
        machines[0].store.receive(snapshot(&["w1:p1", "w1:p2"]));
        machines[1].store.receive(snapshot(&["w1:p1"]));
        let here = &mut machines[0];
        here.selected = Some("w1:p2".into());
        here.expanded = true;
        here.collapsed.insert("shop".into());
        // Pause all runs here when the user switches.
        let mut running = Some("Pausing…");
        assert_eq!(switch(&mut machines, &mut active, &mut running, 1), Some(0));
        assert_eq!((active, running), (1, None));
        assert_eq!(machines[active].store.state.agents.len(), 1);
        assert_eq!(machines[active].selected, None);
        machines[active].selected = Some("w1:p1".into());
        assert_eq!(switch(&mut machines, &mut active, &mut running, 1), None);
        assert_eq!(switch(&mut machines, &mut active, &mut running, 9), None);
        assert_eq!(switch(&mut machines, &mut active, &mut running, 0), Some(1));
        let back = &machines[active];
        assert_eq!(back.selected.as_deref(), Some("w1:p2"));
        assert!(back.expanded && back.collapsed.contains("shop"));
        assert_eq!(back.store.state.agents.len(), 2);
        assert_eq!(running, Some("Pausing…"), "its Pause all is back");
        assert_eq!(machines[1].selected.as_deref(), Some("w1:p1"));
    }

    #[test]
    fn removing_a_machine_keeps_the_one_shown() {
        let (mut machines, mut active) =
            machines_of_run(&options(&[]), &two_saved(Some("peter@box")));
        assert_eq!(active, 2);
        remove(&mut machines, &mut active, 2);
        assert_eq!(machines.len(), 3, "the shown one is switched from first");
        remove(&mut machines, &mut active, 1);
        assert_eq!((machines.len(), active), (2, 1));
        assert_eq!(machines[active].name, "box");
    }

    fn hosts() -> Vec<String> {
        ["omarchy", "box", "devbox"].map(String::from).to_vec()
    }

    #[test]
    fn typing_a_host_and_enter_adds_it_with_every_other_value_preset() {
        let mut form = MachineForm::add(hosts());
        assert_eq!(form.picker_rows().len(), 3, "the list opens at once");
        for c in "omar".chars() {
            assert_eq!(form.key(Key::Char(c)), FormEffect::None);
        }
        let rows = form.picker_rows();
        assert_eq!(rows[0].value, "omarchy");
        assert_eq!(rows.last().unwrap().badge, "as typed");
        assert_eq!(form.key(Key::Enter), FormEffect::Submit);
        assert_eq!(form.host, "omarchy");
        assert_eq!(form.machine(&[], &[]), Some(machine("omarchy", "omarchy")));
    }

    #[test]
    fn a_host_not_in_the_config_is_taken_as_typed() {
        let mut form = MachineForm::add(hosts());
        form.type_text("peter@10.0.0.9");
        assert_eq!(form.picker_rows()[0].badge, "as typed");
        form.key(Key::Enter);
        assert_eq!(form.host, "peter@10.0.0.9");
        assert_eq!(form.name(), "peter@10.0.0.9");
    }

    #[test]
    fn the_rows_move_with_the_arrows_and_tab_and_the_name_follows_the_host_until_typed() {
        let mut form = MachineForm::add(hosts());
        // ↓ moves in the open list; Tab takes its row and moves on.
        form.key(Key::Down);
        assert_eq!(form.key(Key::Tab), FormEffect::None);
        assert_eq!((form.host.as_str(), form.row), ("box", MachineRow::Corgi));
        assert!(form.picker.is_none());
        for _ in 0.."corgi".len() {
            form.key(Key::Backspace);
        }
        form.type_text("~/bin/corgi");
        form.key(Key::Down);
        assert_eq!(form.row, MachineRow::Name);
        assert_eq!(form.shown(MachineRow::Name), "box");
        form.key(Key::Backspace);
        form.type_text("en");
        assert_eq!(form.name(), "boen");
        // Emptied, it follows the host again.
        for _ in 0..4 {
            form.key(Key::Backspace);
        }
        assert_eq!(form.name(), "box");
        form.key(Key::Up);
        form.key(Key::Up);
        form.key(Key::Up);
        assert_eq!(form.row, MachineRow::Host, "the arrows stop at the ends");
        assert_eq!(form.key(Key::BackTab), FormEffect::None);
        assert_eq!(form.row, MachineRow::Name, "Tab wraps");
        assert_eq!(form.key(Key::Escape), FormEffect::Cancel);
    }

    #[test]
    fn a_refused_value_keeps_the_form_open_on_its_row_with_everything_typed() {
        let mut form = MachineForm::add(hosts());
        assert_eq!(form.machine(&[], &[]), None);
        assert_eq!(form.row, MachineRow::Host);
        assert!(form.error.as_deref().unwrap().contains("SSH host"));
        form.type_text("box");
        assert!(form.error.is_none(), "typing clears the refusal");
        let names = vec!["box".to_string()];
        let taken = vec!["box".to_string()];
        assert_eq!(form.machine(&names, &taken), None);
        assert!(
            form.error
                .as_deref()
                .unwrap()
                .contains("in the list already")
        );
        // A name already shown is refused on the name's row.
        form.row = MachineRow::Name;
        form.type_text("");
        assert_eq!(form.machine(&names, &[]), None);
        assert_eq!(form.row, MachineRow::Name);
        form.row = MachineRow::Name;
        form.type_text("2");
        form.row = MachineRow::Corgi;
        for _ in 0..5 {
            form.key(Key::Backspace);
        }
        assert_eq!(form.machine(&names, &[]), None);
        assert_eq!(form.row, MachineRow::Corgi);
        form.type_text("corgi");
        assert_eq!(form.machine(&names, &[]), Some(machine("box2", "box")));
        let mut this = MachineForm::add(hosts());
        this.type_text("x");
        this.row = MachineRow::Name;
        this.type_text("");
        this.name = "This machine".into();
        assert_eq!(this.machine(&[], &[]), None);
        let mut flag = MachineForm::add(hosts());
        flag.type_text("-oProxyCommand=x");
        assert_eq!(flag.machine(&[], &[]), None);
    }

    #[test]
    fn editing_starts_from_the_machine_and_keeps_its_own_host() {
        let saved = SavedMachine {
            corgi: "/opt/corgi".into(),
            ..machine("Omarchy", "omarchy")
        };
        let mut form = MachineForm::edit(&saved, hosts());
        assert_eq!(form.title(), "Edit machine");
        assert!(form.picker.is_none());
        assert_eq!(form.name(), "Omarchy");
        assert_eq!(form.key(Key::Enter), FormEffect::Submit);
        // The window checks against the other machines, not this one.
        assert_eq!(form.machine(&[], &[]), Some(saved));
        let plain = MachineForm::edit(&machine("box", "box"), hosts());
        assert_eq!(plain.name, "", "a name that was the host follows it");
    }
}
