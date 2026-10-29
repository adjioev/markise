//! Native menu bar (macOS: the application's `NSMenu` main menu; Windows: the window's menu
//! bar) built with `muda` from the toolkit-neutral menu model of `markise_ui_egui::menu`.
//!
//! - Items are keyed by [`Item::key`]; chosen items arrive through muda's event handler, which
//!   queues the key and requests a repaint (no polling: idle stays at zero frames, NFR-10).
//!   The app drains the queue at the start of its frame ([`MenuHost::take_actions`]).
//! - [`MenuHost::sync`] runs after each frame: enabled/checked/label changes are applied to the
//!   existing items; the native menus are rebuilt only when the structure changes (theme list,
//!   new registry commands).
//! - macOS draws and routes the key equivalents itself, so the menu performs a shortcut before
//!   egui sees the key (same action as the egui shortcut path). On Windows the accelerators
//!   are shown only: muda's accelerator table needs `TranslateAcceleratorW` in the message
//!   loop, which is not installed, so keys keep reaching egui (no double dispatch).
//! - AppKit inserts its own "Enter/Exit Full Screen" item into any menu titled "View"; the
//!   model therefore has no Full Screen item on macOS (the app's shortcut path keeps ⌃⌘F for
//!   the case the system item does not answer it). Do not add one here.

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, channel};

use markise_ui_egui::MenuHost;
use markise_ui_egui::menu::{Check, Entry, Item, Menu, MenuAction, MenuBar, MenuKind, Role, Trigger, accelerator_text};
use muda::accelerator::Accelerator;
use muda::{CheckMenuItem, IsMenuItem, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};

enum Handle {
    Plain(MenuItem),
    Check(CheckMenuItem),
}

struct State {
    label: String,
    enabled: bool,
}

pub struct NativeMenu {
    menu: Option<muda::Menu>,
    structure: String,
    items: HashMap<String, (Handle, State)>,
    actions: HashMap<String, MenuAction>,
    /// Submenus in depth-first order with their enabled state.
    submenus: Vec<(Submenu, bool)>,
    rx: Receiver<(String, Trigger)>,
    #[cfg(windows)]
    hwnd: isize,
}

impl NativeMenu {
    /// Prepares the native menu bar for the window of `cc`; menus appear with the first
    /// [`MenuHost::sync`]. `None` if the platform window is not available.
    pub fn new(cc: &eframe::CreationContext<'_>) -> Option<NativeMenu> {
        #[cfg(windows)]
        let hwnd = {
            use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
            match cc.window_handle().ok()?.as_raw() {
                RawWindowHandle::Win32(h) => h.hwnd.get(),
                _ => return None,
            }
        };
        let (tx, rx) = channel();
        let ctx = cc.egui_ctx.clone();
        MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
            let _ = tx.send((e.id.0, trigger()));
            ctx.request_repaint();
        }));
        Some(NativeMenu {
            menu: None,
            structure: String::new(),
            items: HashMap::new(),
            actions: HashMap::new(),
            submenus: Vec::new(),
            rx,
            #[cfg(windows)]
            hwnd,
        })
    }

    fn rebuild(&mut self, bar: &MenuBar) {
        self.items.clear();
        self.actions.clear();
        self.submenus.clear();
        let menu = muda::Menu::new();
        for m in &bar.menus {
            if !cfg!(target_os = "macos") && matches!(m.kind, MenuKind::App | MenuKind::Window) {
                continue;
            }
            let sub = self.submenu(m);
            let _ = menu.append(&sub);
            #[cfg(target_os = "macos")]
            match m.kind {
                MenuKind::Window => sub.set_as_windows_menu_for_nsapp(),
                MenuKind::Help => sub.set_as_help_menu_for_nsapp(),
                _ => {}
            }
        }
        #[cfg(target_os = "macos")]
        menu.init_for_nsapp();
        #[cfg(windows)]
        // SAFETY: `hwnd` is the app's live top-level window (from the creation context); the
        // menu is attached to it on the UI thread that owns it, and the previous menu is
        // detached first.
        unsafe {
            if let Some(old) = &self.menu {
                let _ = old.remove_for_hwnd(self.hwnd);
            }
            if let Err(e) = menu.init_for_hwnd(self.hwnd) {
                eprintln!("markise: native menu bar failed: {e}");
            }
        }
        self.menu = Some(menu);
    }

    fn submenu(&mut self, m: &Menu) -> Submenu {
        let sub = Submenu::new(esc(&m.title), m.enabled);
        self.submenus.push((sub.clone(), m.enabled));
        for e in &m.entries {
            match e {
                Entry::Item(i) => {
                    let h = self.item(i);
                    let _ = match &h {
                        Handle::Plain(x) => sub.append(x),
                        Handle::Check(x) => sub.append(x),
                    };
                    self.actions.insert(i.key.clone(), i.action.clone());
                    self.items.insert(
                        i.key.clone(),
                        (
                            h,
                            State {
                                label: i.label.clone(),
                                enabled: i.enabled,
                            },
                        ),
                    );
                }
                Entry::Separator => {
                    let _ = sub.append(&PredefinedMenuItem::separator());
                }
                Entry::Header(h) => {
                    let _ = sub.append(&MenuItem::new(esc(h), false, None));
                }
                Entry::Submenu(child) => {
                    let c = self.submenu(child);
                    let _ = sub.append(&c);
                }
                Entry::Role(r) => {
                    if let Some(p) = role(*r) {
                        let _ = sub.append(&p as &dyn IsMenuItem);
                    }
                }
            }
        }
        sub
    }

    fn item(&self, i: &Item) -> Handle {
        let accel = i
            .chord
            .as_ref()
            .and_then(accelerator_text)
            .and_then(|t| t.parse::<Accelerator>().ok());
        match i.check {
            Check::None => Handle::Plain(MenuItem::with_id(i.key.as_str(), esc(&i.label), i.enabled, accel)),
            Check::Toggle(on) | Check::Radio(on) => Handle::Check(CheckMenuItem::with_id(
                i.key.as_str(),
                esc(&i.label),
                i.enabled,
                on,
                accel,
            )),
        }
    }

    fn update(&mut self, bar: &MenuBar) {
        for i in bar.items() {
            let Some((h, s)) = self.items.get_mut(&i.key) else {
                continue;
            };
            if s.label != i.label {
                match h {
                    Handle::Plain(x) => x.set_text(esc(&i.label)),
                    Handle::Check(x) => x.set_text(esc(&i.label)),
                }
                s.label = i.label.clone();
            }
            if s.enabled != i.enabled {
                match h {
                    Handle::Plain(x) => x.set_enabled(i.enabled),
                    Handle::Check(x) => x.set_enabled(i.enabled),
                }
                s.enabled = i.enabled;
            }
            // against the native state: muda flips a check item itself when it is clicked, so
            // clicking the selected radio item would otherwise lose its check mark
            if let Handle::Check(x) = h
                && let Some(on) = check_fix(x.is_checked(), i.check)
            {
                x.set_checked(on);
            }
            self.actions.insert(i.key.clone(), i.action.clone());
        }
        let mut enabled = Vec::with_capacity(self.submenus.len());
        fn walk(m: &Menu, out: &mut Vec<bool>) {
            out.push(m.enabled);
            for e in &m.entries {
                if let Entry::Submenu(c) = e {
                    walk(c, out);
                }
            }
        }
        for m in &bar.menus {
            if cfg!(target_os = "macos") || !matches!(m.kind, MenuKind::App | MenuKind::Window) {
                walk(m, &mut enabled);
            }
        }
        for ((sub, was), now) in self.submenus.iter_mut().zip(enabled) {
            if *was != now {
                sub.set_enabled(now);
                *was = now;
            }
        }
    }
}

/// The checked state to set on a native check item showing `native` for the model's `check`,
/// if they differ.
fn check_fix(native: bool, check: Check) -> Option<bool> {
    match check {
        Check::Toggle(on) | Check::Radio(on) if on != native => Some(on),
        _ => None,
    }
}

/// How the current menu event was triggered: on macOS the item's key equivalent while the
/// application handles a key-down event, else a click. Windows menus never route
/// accelerators here (see the module comment), so they are always clicks.
fn trigger() -> Trigger {
    #[cfg(target_os = "macos")]
    {
        use objc2::msg_send;
        use objc2::runtime::{AnyClass, AnyObject};
        const NS_EVENT_TYPE_KEY_DOWN: usize = 10;
        let Some(cls) = AnyClass::get(c"NSApplication") else {
            return Trigger::Click;
        };
        // SAFETY: muda calls the handler on the main thread from the menu item's action;
        // `sharedApplication`, `currentEvent` and `type` exist with these return types.
        unsafe {
            let app: *mut AnyObject = msg_send![cls, sharedApplication];
            let Some(app) = app.as_ref() else { return Trigger::Click };
            let ev: *mut AnyObject = msg_send![app, currentEvent];
            if let Some(ev) = ev.as_ref() {
                let ty: usize = msg_send![ev, type];
                if ty == NS_EVENT_TYPE_KEY_DOWN {
                    return Trigger::Shortcut;
                }
            }
        }
    }
    Trigger::Click
}

/// muda treats `&` as a mnemonic marker (Windows access keys; stripped on macOS): a literal
/// `&` ("Font & Size…") is written `&&`.
fn esc(label: &str) -> String {
    label.replace('&', "&&")
}

fn role(r: Role) -> Option<PredefinedMenuItem> {
    Some(match r {
        Role::About => PredefinedMenuItem::about(
            Some("About Markise"),
            Some(muda::AboutMetadata {
                name: Some("Markise".into()),
                version: Some(env!("CARGO_PKG_VERSION").into()),
                ..Default::default()
            }),
        ),
        Role::Services => PredefinedMenuItem::services(None),
        Role::Hide => PredefinedMenuItem::hide(Some("Hide Markise")),
        Role::HideOthers => PredefinedMenuItem::hide_others(None),
        Role::ShowAll => PredefinedMenuItem::show_all(None),
        Role::Minimize => PredefinedMenuItem::minimize(None),
        Role::Zoom => PredefinedMenuItem::maximize(Some("Zoom")),
        Role::BringAllToFront => PredefinedMenuItem::bring_all_to_front(None),
    })
}

impl MenuHost for NativeMenu {
    fn take_actions(&mut self) -> Vec<(MenuAction, Trigger)> {
        self.rx
            .try_iter()
            .filter_map(|(key, t)| Some((self.actions.get(&key).cloned()?, t)))
            .collect()
    }

    fn sync(&mut self, bar: &MenuBar) {
        let st = bar.structure();
        if st != self.structure || self.menu.is_none() {
            self.rebuild(bar);
            self.structure = st;
        } else {
            self.update(bar);
        }
    }
}

#[cfg(test)]
mod tests {
    use markise_ui_egui::menu::{Platform, accelerator_text};

    /// Every shortcut of the menu model (both platforms) is expressible as a native accelerator.
    /// Clicking the selected radio item makes muda clear its check mark; the next sync must
    /// restore it although the model did not change (Codex review of PR #34).
    #[test]
    fn check_marks_follow_the_model_not_the_last_sync() {
        use markise_ui_egui::menu::Check;
        assert_eq!(super::check_fix(false, Check::Radio(true)), Some(true));
        assert_eq!(super::check_fix(true, Check::Radio(false)), Some(false));
        assert_eq!(super::check_fix(true, Check::Toggle(true)), None);
        assert_eq!(super::check_fix(true, Check::None), None);
    }

    #[test]
    fn ampersands_are_escaped_for_muda() {
        assert_eq!(super::esc("Font & Size…"), "Font && Size…");
    }

    #[test]
    fn model_accelerators_parse_as_native_accelerators() {
        let mut app = markise_ui_egui::MarkiseApp::new(Default::default());
        for p in [Platform::Mac, Platform::Other] {
            app.set_menu_platform(p);
            for i in app.menu_model().items() {
                if let Some(c) = &i.chord {
                    let t = accelerator_text(c).unwrap_or_else(|| panic!("{}: {c:?}", i.key));
                    assert!(t.parse::<muda::accelerator::Accelerator>().is_ok(), "{}: {t}", i.key);
                }
            }
        }
    }
}
