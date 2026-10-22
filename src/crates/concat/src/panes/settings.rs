// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Jareer Abdullah and Concat contributors

//! The settings sheet: the preferences, and what follows from them.
//!
//! Everything on the sheet is a preference or follows from one, so most
//! messages do three things: change the pane, write the preference, and
//! apply it - to the downloaders, to the words on screen. The Version
//! page is the one part with a worker behind it: a switch to another
//! release reports as [`SettingsMsg::InstallProgress`] and ends as
//! [`SettingsMsg::InstallFinished`].

use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use concat_host::models::SourcePreference;
use concat_host::updates::{self, Fixed, Installed, Release, Standing, Version};
use slint::{ModelRc, SharedString, VecModel};

use crate::close_window;
use crate::host::{on_ui, spawn};
use crate::i18n::{self, t, tf};
use crate::keymap;
use crate::panes::Msg;
use crate::platform;
use crate::studio::Studio;
use crate::ui::{SettingsData, ShortcutData};

/// Everything that can happen to the settings sheet.
#[derive(Clone, Debug)]
pub enum SettingsMsg {
    /// The preferences read back on launch, and applied.
    Restore,
    /// The sheet is asked for: the menu, the tray or the shortcut.
    Open,
    Close,
    PageChanged(i32),
    /// The Shortcuts page: a row asked to hold the next key it should
    /// be bound to. The sheet closing puts the listening down too — see
    /// `Close` — so no key anywhere binds something the page stopped
    /// caring about.
    ShortcutCapture(String),
    /// One row put back as it ships, taking its chord off whoever holds
    /// it now, and every row put back at once.
    ShortcutReset(String),
    ShortcutResetAll,
    /// Show this run's log in the file manager.
    ShowLog,
    LanguageChanged(i32),
    /// The log level's row: an index into [`LOG_LEVELS`].
    LogLevelChanged(i32),
    PlayheadStopsChanged(bool),
    /// The proxies switch: whether large files get a smaller copy to play.
    ProxiesChanged(bool),
    /// The GPU switch: on, the window and the compositor draw on the GPU;
    /// off, in software. Read at the next launch.
    GpuChanged(bool),
    /// The magnetic timeline switch; the tray's button is the same fact.
    MagneticChanged(bool),
    DownloadSourceChanged(i32),
    DownloadBaseEdited(String),
    /// The Version page asks GitHub for the releases.
    CheckUpdates,
    /// The releases, newest first, or why they could not be had.
    UpdatesFetched(Result<Vec<Release>, String>),
    /// A release picked from the Version page's list.
    VersionChosen(i32),
    /// Put the picked release in this one's place.
    InstallVersion,
    InstallCancel,
    /// The switch's worker reporting its download, in megabytes.
    InstallProgress {
        fetched: f32,
        total: f32,
    },
    /// The switch's worker is done: what it did, or why not.
    InstallFinished {
        version: Version,
        result: Result<Installed, String>,
    },
}

/// The Version page's index in the column of pages.
const VERSION_PAGE: i32 = 6;
/// The Shortcuts page's index, which the launch screen's Keyboard
/// shortcuts card opens the sheet on.
pub const SHORTCUTS_PAGE: i32 = 7;

/// The settings sheet's state.
#[derive(Default)]
pub struct SettingsPane {
    pub open: bool,
    pub tab: i32,
    /// The Shortcuts page's row listening for its next key: the action
    /// that key will bind to. None when none is — and when the sheet
    /// closes under a listening row, so a key pressed in the editor
    /// afterwards dispatches rather than binding.
    pub capturing: Option<String>,
    pub language: usize,
    /// The switch that keeps the playhead inside the content.
    pub playhead_stops: bool,
    /// Index into `SourcePreference::ALL`: where model downloads look first.
    pub download_source: usize,
    /// The base URL of a custom download source.
    pub download_base: String,
    /// The Version page's list: the stable releases from 0.2.5 on, newest
    /// first, once GitHub has been asked.
    pub releases: Vec<Release>,
    /// Which of `releases` the page's dropdown shows.
    pub version_choice: usize,
    /// What the Version page says under the version: looking, up to date,
    /// a newer release, or why it could not look.
    pub update_status: String,
    /// GitHub has been asked and has not answered yet.
    pub checking: bool,
    /// GitHub has answered this run: the page asks once when it is opened,
    /// and again only when the button is pressed.
    checked: bool,
    /// A switch in flight: megabytes fetched, and megabytes in all once
    /// the server has said.
    install: Option<(f32, f32)>,
    /// The running switch's stop flag.
    install_cancel: Arc<AtomicBool>,
}

impl SettingsPane {
    /// Applies one message. The studio is the rest of the window; while
    /// this runs the studio's copy of the pane is a blank it must not read.
    pub fn update(&mut self, msg: SettingsMsg, studio: &mut Studio) {
        match msg {
            SettingsMsg::Restore => {
                self.language = studio
                    .languages
                    .iter()
                    .position(|language| {
                        Some(language.code.as_str()) == studio.prefs.locale.as_deref()
                    })
                    .unwrap_or(0);
                self.playhead_stops = studio.prefs.playhead_stops_at_end;
                // Not a choice: the app decodes on whatever hardware the
                // platform has, and a file the hardware will not take opens
                // in software (concat_media::hardware). Linux has no default
                // device, so there this is software as before.
                concat_media::set_hardware_decode(true);
                self.download_source = Self::download_source(studio).0;
                self.download_base = studio.prefs.download_base.clone().unwrap_or_default();
                Self::apply_download_source(studio);
            }
            SettingsMsg::Open => {
                self.open = true;
            }
            SettingsMsg::Close => {
                // A row left listening when the sheet goes would take
                // the next key anywhere in the window; it is put down
                // with the sheet.
                self.capturing = None;
                self.open = false;
            }
            SettingsMsg::PageChanged(index) => {
                self.tab = index;
                // The Version page looks for the releases the first time it
                // is opened in a run, so it has something to say and to list
                // without a press; the button looks again.
                if index == VERSION_PAGE && !self.checked {
                    self.check_updates();
                }
            }
            SettingsMsg::ShortcutCapture(id) => self.capturing = Some(id),
            SettingsMsg::ShortcutReset(id) => {
                self.capturing = None;
                let lost = keymap::reset(&mut studio.prefs, &id);
                studio.prefs.save(&studio.host.dirs);
                // The shipped chord came back off whoever was holding
                // it, and that row now has no key until it is reset.
                if let Some(lost) = lost {
                    studio.notify(&tf("shortcuts.unboundNow", &[&keymap::name(lost)]), false);
                }
            }
            SettingsMsg::ShortcutResetAll => {
                self.capturing = None;
                keymap::reset_all(&mut studio.prefs);
                studio.prefs.save(&studio.host.dirs);
            }
            SettingsMsg::ShowLog => {
                // The file this run is writing, when there is one, so the
                // manager opens with it selected; the folder when there is
                // not, which is still where the previous runs are.
                let target = concat_host::logs::current()
                    .map(std::path::Path::to_path_buf)
                    .unwrap_or_else(|| concat_host::logs::folder(&studio.host.dirs));
                if let Err(error) = platform::reveal(&target.to_string_lossy()) {
                    studio.notify(&i18n::tf("settings.couldNotShowLog", &[&error]), true);
                }
            }
            SettingsMsg::LanguageChanged(index) => {
                let index = index.max(0) as usize;
                if let Some(language) = studio.languages.get(index).cloned() {
                    self.language = index;
                    studio.prefs.locale = Some(language.code.clone());
                    // The words change on the publish that follows: Rust's
                    // on their way through `t`, the tree's through
                    // `I18n.lang`.
                    i18n::select(&language.code, &studio.host.dirs);
                }
                studio.prefs.save(&studio.host.dirs);
            }
            SettingsMsg::LogLevelChanged(index) => {
                let Some(level) = LOG_LEVELS.get(index.max(0) as usize) else {
                    return;
                };
                studio.prefs.log_level = Some(concat_host::logs::level_name(*level).to_owned());
                studio.prefs.save(&studio.host.dirs);
                // Applied now, whatever the environment said: a person at
                // the sheet is asking for this run too.
                concat_host::logs::set_level(*level);
            }
            SettingsMsg::PlayheadStopsChanged(on) => {
                self.playhead_stops = on;
                studio.prefs.playhead_stops_at_end = on;
                studio.prefs.save(&studio.host.dirs);
                // A playhead already out past the end comes back in when
                // the switch goes on; seek does the clamp.
                let at = studio.playhead;
                studio.seek(at);
            }
            SettingsMsg::ProxiesChanged(on) => {
                studio.prefs.proxies = on;
                studio.prefs.save(&studio.host.dirs);
                // On, the media already in the project gets its proxies
                // straight away (refresh_art's stamp holds the switch);
                // off, playback reads the files from the next frame.
            }
            SettingsMsg::GpuChanged(on) => {
                studio.prefs.software_gpu = !on;
                studio.prefs.software_after_crash = None;
                studio.prefs.save(&studio.host.dirs);
                studio.notify(&t("settings.gpuNextStart"), false);
            }
            SettingsMsg::MagneticChanged(on) => {
                studio.prefs.magnetic = on;
                studio.prefs.save(&studio.host.dirs);
            }
            SettingsMsg::DownloadSourceChanged(index) => {
                let index = (index.max(0) as usize).min(SourcePreference::ALL.len() - 1);
                self.download_source = index;
                studio.prefs.download_source = Some(SourcePreference::ALL[index].name().to_owned());
                studio.prefs.save(&studio.host.dirs);
                Self::apply_download_source(studio);
            }
            SettingsMsg::DownloadBaseEdited(text) => {
                let base = text.trim().to_owned();
                self.download_base = base.clone();
                studio.prefs.download_base = (!base.is_empty()).then_some(base);
                studio.prefs.save(&studio.host.dirs);
                Self::apply_download_source(studio);
            }
            SettingsMsg::CheckUpdates => self.check_updates(),
            SettingsMsg::UpdatesFetched(result) => {
                self.checking = false;
                match result {
                    Ok(releases) => {
                        self.checked = true;
                        let current = Version::current();
                        self.update_status = match updates::standing(current, &releases) {
                            Standing::Latest => tf("settings.upToDate", &[&current]),
                            Standing::Behind(newer) => {
                                tf("settings.updateAvailable", &[&newer, &current])
                            }
                            Standing::Ahead => tf("settings.aheadOfReleases", &[&current]),
                            Standing::NoReleases => t("settings.noReleasesYet"),
                        };
                        // The list starts on its newest release, which is
                        // the one most people opened the page for.
                        self.version_choice = 0;
                        self.releases = releases;
                    }
                    Err(error) => self.update_status = tf("settings.couldNotCheck", &[&error]),
                }
            }
            SettingsMsg::VersionChosen(index) => {
                self.version_choice =
                    (index.max(0) as usize).min(self.releases.len().saturating_sub(1));
            }
            SettingsMsg::InstallVersion => self.install_version(studio),
            SettingsMsg::InstallCancel => self.install_cancel.store(true, Ordering::Relaxed),
            SettingsMsg::InstallProgress { fetched, total } => {
                if self.install.is_some() {
                    self.install = Some((fetched, total));
                }
            }
            SettingsMsg::InstallFinished { version, result } => {
                self.install = None;
                match result {
                    Ok(Installed::InstallerRunning) => {
                        log::info!("update: the installer for {version} is running; closing");
                        studio.notify(&tf("settings.installerOpened", &[&version]), false);
                        quit_for_switch(|| {});
                    }
                    Ok(Installed::Replaced) => {
                        // The bundle on disk is already the new build; what
                        // is left is to start it, and that waits on the
                        // project being saved.
                        log::info!("update: {version} is in place; starting it");
                        studio.notify(&tf("settings.versionInstalled", &[&version]), false);
                        quit_for_switch(|| {
                            if let Err(error) = updates::relaunch() {
                                log::warn!("update: could not arrange the start: {error}");
                            }
                        });
                    }
                    Ok(Installed::Opened) => {
                        studio.notify(&tf("settings.packageOpened", &[&version]), false);
                    }
                    // A cancel is a choice, not a failure worth a toast.
                    Err(_) if self.install_cancel.load(Ordering::Relaxed) => {}
                    Err(error) => {
                        log::warn!("update: {version} could not be installed: {error}");
                        studio.notify(&tf("settings.couldNotInstall", &[&version, &error]), true);
                    }
                }
            }
        }
    }

    /// Asks GitHub for the releases on a worker; the answer comes back as
    /// [`SettingsMsg::UpdatesFetched`].
    fn check_updates(&mut self) {
        if self.checking {
            return;
        }
        self.checking = true;
        self.update_status = t("settings.checkingForUpdates");
        spawn(updates::fetch_releases, |studio, _, _, result| {
            studio.handle(Msg::Settings(SettingsMsg::UpdatesFetched(result)));
        });
    }

    /// Fetches the chosen release's package for this machine on a worker,
    /// checks it against the release's digest, and puts it in place. The
    /// download reports as [`SettingsMsg::InstallProgress`] and the whole
    /// ends as [`SettingsMsg::InstallFinished`].
    fn install_version(&mut self, studio: &mut Studio) {
        if self.install.is_some() {
            return;
        }
        let Some(release) = self.releases.get(self.version_choice).cloned() else {
            return;
        };
        let kind = match updates::installed_as() {
            Ok(kind) => kind,
            Err(why) => {
                studio.notify(&fixed_note(why), true);
                return;
            }
        };
        let cancel = Arc::new(AtomicBool::new(false));
        self.install_cancel = Arc::clone(&cancel);
        self.install = Some((0.0, 0.0));
        let folder = updates::folder(&studio.host.dirs);
        let version = release.version;
        log::info!(
            "update: switching to {version} by its {} package",
            kind.key()
        );
        spawn(
            move || {
                let result = updates::fetch_package(&release, kind)
                    .and_then(|package| {
                        let mut report = |received: u64, total: u64| {
                            let msg = SettingsMsg::InstallProgress {
                                fetched: received as f32 / 1_000_000.0,
                                total: total as f32 / 1_000_000.0,
                            };
                            on_ui(move |studio, _, _| studio.handle(Msg::Settings(msg)));
                        };
                        updates::download_package(&package, &folder, &cancel, &mut report)
                    })
                    .and_then(|path| updates::install(&path, kind));
                (version, result)
            },
            |studio, _, _, (version, result)| {
                studio.handle(Msg::Settings(SettingsMsg::InstallFinished {
                    version,
                    result,
                }));
            },
        );
    }

    /// The Version page's list: each release's number.
    pub fn version_rows(&self) -> Vec<SharedString> {
        self.releases
            .iter()
            .map(|release| release.version.to_string().into())
            .collect()
    }

    /// Beside each release: the day it came out, or that it is this one.
    pub fn version_details(&self) -> Vec<SharedString> {
        let current = Version::current();
        self.releases
            .iter()
            .map(|release| {
                if release.version == current {
                    t("settings.thisVersion").into()
                } else {
                    release.published.as_str().into()
                }
            })
            .collect()
    }

    /// The remembered download source: its place in the menu, and itself.
    pub fn download_source(studio: &Studio) -> (usize, SourcePreference) {
        let preference =
            SourcePreference::parse(studio.prefs.download_source.as_deref().unwrap_or_default());
        let index = SourcePreference::ALL
            .iter()
            .position(|candidate| *candidate == preference)
            .unwrap_or(0);
        (index, preference)
    }

    /// Tells the downloaders where to look first, from the preferences.
    fn apply_download_source(studio: &Studio) {
        let (_, preference) = Self::download_source(studio);
        concat_host::models::set_preference(
            preference,
            studio.prefs.download_base.as_deref().unwrap_or_default(),
        );
    }

    /// The sheet as Slint shows it.
    pub fn data(&self, studio: &Studio) -> SettingsData {
        SettingsData {
            open: self.open,
            tab: self.tab,
            // The keymap's rows as the page draws them: the chord each
            // action is on now, and whether that moved from the one it
            // ships on — which is the row's cue to offer its reset.
            shortcuts: ModelRc::from(Rc::new(VecModel::from(
                keymap::ACTIONS
                    .iter()
                    .map(|action| ShortcutData {
                        id: action.id.into(),
                        label: keymap::name(action.id).into(),
                        keys: keymap::bound(&studio.prefs, action.id)
                            .map(keymap::display)
                            .unwrap_or_default()
                            .into(),
                        custom: studio.prefs.keybinds.contains_key(action.id),
                    })
                    .collect::<Vec<_>>(),
            ))),
            capturing: self.capturing.clone().unwrap_or_default().into(),
            language: self.language as i32,
            log_level: LOG_LEVELS
                .iter()
                .position(|level| *level == log::max_level())
                .unwrap_or(2) as i32,
            playhead_stops: self.playhead_stops,
            proxies: studio.prefs.proxies,
            gpu: !studio.prefs.draws_in_software(),
            gpu_switchable: cfg!(any(windows, target_os = "linux")),
            magnetic: studio.prefs.magnetic,
            download_source: self.download_source as i32,
            download_base: self.download_base.as_str().into(),
            version: env!("CARGO_PKG_VERSION").into(),
            update_status: self.update_status.as_str().into(),
            checking: self.checking,
            version_choice: self.version_choice as i32,
            installing: self.install.is_some(),
            install_progress: match self.install {
                Some((fetched, total)) if total > 0.0 => (fetched / total).min(1.0),
                _ => 0.0,
            },
            install_transferred: match self.install {
                Some((fetched, total)) if total > 0.0 => tf(
                    "settings.mbOfMb",
                    &[&format!("{fetched:.0}"), &format!("{total:.0}")],
                ),
                Some(_) => tf(
                    "settings.downloadingVersion",
                    &[&self
                        .releases
                        .get(self.version_choice)
                        .map(|release| release.version.to_string())
                        .unwrap_or_default()],
                ),
                None => String::new(),
            }
            .into(),
            can_install: self.install.is_none()
                && !self.releases.is_empty()
                && updates::installed_as().is_ok(),
            install_note: updates::installed_as()
                .err()
                .map(fixed_note)
                .unwrap_or_default()
                .into(),
        }
    }
}

/// Why the Install button is off, in words.
fn fixed_note(why: Fixed) -> String {
    match why {
        Fixed::Flatpak => t("settings.fromFlatpak"),
        Fixed::Store => t("settings.fromStore"),
        Fixed::NotFromPackage => t("settings.notFromPackage"),
    }
}

/// Saves and closes the project, then quits - running `after`, the start of
/// a build already replaced on disk, only once the project has been written.
///
/// A switch is only worth making if the save lands first. The relaunch of a
/// replaced build used to be scheduled before the save, so a save that then
/// failed started the new build over a window that still held the old
/// session. Now a failed save starts nothing and quits nothing: the window
/// stays up with `close_project`'s reason, and the bundle already on disk
/// can be started by hand.
fn quit_for_switch(after: impl FnOnce() + Send + 'static) {
    on_ui(move |studio, app, models| {
        log::info!("close: quitting for a version switch");
        let outcome = close_window(
            || studio.close_project(),
            || {
                after();
                slint::ComponentHandle::window(app).hide().ok();
                slint::quit_event_loop().ok();
            },
        );
        if let Err(error) = outcome {
            log::warn!("close: not switching, project could not be saved: {error}");
            studio.offer_quit_without_saving(error);
            studio.refresh_art();
            studio.publish(app, models);
        }
    });
}

/// The log level row's choices, in the order the sheet lists them:
/// errors only, warnings, normal, debug, everything.
/// https://github.com/jub0t/Concat/issues/223
const LOG_LEVELS: [log::LevelFilter; 5] = [
    log::LevelFilter::Error,
    log::LevelFilter::Warn,
    log::LevelFilter::Info,
    log::LevelFilter::Debug,
    log::LevelFilter::Trace,
];
