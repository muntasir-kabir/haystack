//! Application-wide keyboard command registry.
//!
//! Keeping shortcut definitions here makes the command palette and help sheet
//! truthful by construction: they render the same commands this listener
//! consumes.

use eframe::egui;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AppCommand {
    Open,
    CloseTab,
    NextTab,
    PreviousTab,
    FocusFind,
    GoTo,
    NextFind,
    PreviousFind,
    ViewportStart,
    ViewportEnd,
    DocumentStart,
    DocumentEnd,
    ToggleLane,
    DeleteSelected,
    UndoDelete,
    ToggleLogFocus,
    ShowHelp,
    ShowPalette,
}

#[derive(Clone, Copy)]
pub(super) struct CommandSpec {
    pub command: AppCommand,
    pub name: &'static str,
    pub category: &'static str,
    pub shortcut: Option<egui::KeyboardShortcut>,
    pub text_safe: bool,
}

const COMMAND: egui::Modifiers = egui::Modifiers::COMMAND;
const CTRL: egui::Modifiers = egui::Modifiers::CTRL;
const SHIFT: egui::Modifiers = egui::Modifiers::SHIFT;
const CTRL_SHIFT: egui::Modifiers = egui::Modifiers {
    ctrl: true,
    shift: true,
    ..egui::Modifiers::NONE
};
const COMMAND_SHIFT: egui::Modifiers = egui::Modifiers {
    command: true,
    shift: true,
    ..egui::Modifiers::NONE
};

pub(super) const COMMANDS: &[CommandSpec] = &[
    CommandSpec {
        command: AppCommand::Open,
        name: "Open file",
        category: "File",
        shortcut: Some(egui::KeyboardShortcut::new(COMMAND, egui::Key::O)),
        text_safe: true,
    },
    CommandSpec {
        command: AppCommand::CloseTab,
        name: "Close tab",
        category: "File",
        shortcut: Some(egui::KeyboardShortcut::new(COMMAND, egui::Key::W)),
        text_safe: true,
    },
    CommandSpec {
        command: AppCommand::NextTab,
        name: "Next tab",
        category: "Navigation",
        shortcut: Some(egui::KeyboardShortcut::new(CTRL, egui::Key::Tab)),
        text_safe: true,
    },
    CommandSpec {
        command: AppCommand::PreviousTab,
        name: "Previous tab",
        category: "Navigation",
        shortcut: Some(egui::KeyboardShortcut::new(CTRL_SHIFT, egui::Key::Tab)),
        text_safe: true,
    },
    CommandSpec {
        command: AppCommand::FocusFind,
        name: "Find",
        category: "Search",
        shortcut: Some(egui::KeyboardShortcut::new(COMMAND, egui::Key::F)),
        text_safe: true,
    },
    CommandSpec {
        command: AppCommand::GoTo,
        name: "Go to line or time",
        category: "Navigation",
        shortcut: Some(egui::KeyboardShortcut::new(COMMAND, egui::Key::L)),
        text_safe: true,
    },
    CommandSpec {
        command: AppCommand::NextFind,
        name: "Next search result",
        category: "Search",
        shortcut: Some(egui::KeyboardShortcut::new(
            egui::Modifiers::NONE,
            egui::Key::F3,
        )),
        text_safe: false,
    },
    CommandSpec {
        command: AppCommand::PreviousFind,
        name: "Previous search result",
        category: "Search",
        shortcut: Some(egui::KeyboardShortcut::new(SHIFT, egui::Key::F3)),
        text_safe: false,
    },
    CommandSpec {
        command: AppCommand::ViewportStart,
        name: "Top of viewport",
        category: "Navigation",
        shortcut: Some(egui::KeyboardShortcut::new(
            egui::Modifiers::NONE,
            egui::Key::Home,
        )),
        text_safe: false,
    },
    CommandSpec {
        command: AppCommand::ViewportEnd,
        name: "Bottom of viewport",
        category: "Navigation",
        shortcut: Some(egui::KeyboardShortcut::new(
            egui::Modifiers::NONE,
            egui::Key::End,
        )),
        text_safe: false,
    },
    CommandSpec {
        command: AppCommand::DocumentStart,
        name: "First visible line",
        category: "Navigation",
        shortcut: Some(egui::KeyboardShortcut::new(COMMAND, egui::Key::Home)),
        text_safe: false,
    },
    CommandSpec {
        command: AppCommand::DocumentEnd,
        name: "Last visible line",
        category: "Navigation",
        shortcut: Some(egui::KeyboardShortcut::new(COMMAND, egui::Key::End)),
        text_safe: false,
    },
    CommandSpec {
        command: AppCommand::ToggleLane,
        name: "Toggle selected filter lane",
        category: "Filters & Pins",
        shortcut: Some(egui::KeyboardShortcut::new(
            egui::Modifiers::NONE,
            egui::Key::Space,
        )),
        text_safe: false,
    },
    CommandSpec {
        command: AppCommand::DeleteSelected,
        name: "Delete selected filter or pin",
        category: "Filters & Pins",
        shortcut: Some(egui::KeyboardShortcut::new(
            egui::Modifiers::NONE,
            egui::Key::Delete,
        )),
        text_safe: false,
    },
    CommandSpec {
        command: AppCommand::UndoDelete,
        name: "Undo deletion",
        category: "Filters & Pins",
        shortcut: Some(egui::KeyboardShortcut::new(COMMAND, egui::Key::Z)),
        text_safe: false,
    },
    CommandSpec {
        command: AppCommand::ToggleLogFocus,
        name: "Focus Log View",
        category: "View",
        shortcut: None,
        text_safe: false,
    },
    CommandSpec {
        command: AppCommand::ShowHelp,
        name: "Keyboard and mouse shortcuts",
        category: "Help",
        shortcut: Some(egui::KeyboardShortcut::new(SHIFT, egui::Key::Slash)),
        text_safe: false,
    },
    CommandSpec {
        command: AppCommand::ShowPalette,
        name: "Show command palette",
        category: "Help",
        shortcut: Some(egui::KeyboardShortcut::new(COMMAND_SHIFT, egui::Key::P)),
        text_safe: true,
    },
];

/// Additional bindings for a command that should appear only once in menus.
/// The canonical command entry remains the palette/cheat-sheet row.
pub(super) const ALIASES: &[(AppCommand, egui::KeyboardShortcut)] = &[(
    AppCommand::GoTo,
    egui::KeyboardShortcut::new(COMMAND, egui::Key::G),
)];

pub(super) fn consume(ctx: &egui::Context, text_edit_active: bool) -> Vec<AppCommand> {
    // Read this before entering `input_mut`: calling any other Context method
    // from inside that closure would try to re-enter egui's context lock.
    let is_mac = ctx.os() == egui::os::OperatingSystem::Mac;
    consume_with_platform(ctx, text_edit_active, is_mac)
}

fn consume_with_platform(
    ctx: &egui::Context,
    text_edit_active: bool,
    is_mac: bool,
) -> Vec<AppCommand> {
    ctx.input_mut(|input| {
        let mut commands: Vec<_> = COMMANDS
            .iter()
            .filter(|spec| !text_edit_active || spec.text_safe)
            .filter_map(|spec| {
                spec.shortcut
                    .filter(|shortcut| input.consume_shortcut(shortcut))
                    .map(|_| spec.command)
            })
            .collect();
        commands.extend(
            ALIASES
                .iter()
                .filter(|(command, _)| {
                    !text_edit_active
                        || COMMANDS
                            .iter()
                            .find(|spec| spec.command == *command)
                            .is_some_and(|spec| spec.text_safe)
                })
                .filter_map(|(command, shortcut)| {
                    input.consume_shortcut(shortcut).then_some(*command)
                }),
        );
        // macOS labels the physical backspace key “Delete”; accepting both
        // variants keeps the advertised shortcut usable on every platform.
        if !text_edit_active
            && is_mac
            && input.consume_key(egui::Modifiers::NONE, egui::Key::Backspace)
        {
            commands.push(AppCommand::DeleteSelected);
        }
        commands
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_contains_the_conventional_navigation_commands_once() {
        for command in [
            AppCommand::Open,
            AppCommand::CloseTab,
            AppCommand::NextTab,
            AppCommand::PreviousTab,
            AppCommand::GoTo,
            AppCommand::NextFind,
            AppCommand::PreviousFind,
            AppCommand::ShowHelp,
        ] {
            assert_eq!(
                COMMANDS
                    .iter()
                    .filter(|spec| spec.command == command)
                    .count(),
                1
            );
        }
    }

    #[test]
    fn consume_does_not_reenter_context_lock_on_any_platform() {
        // This calls the real platform-detecting entry point. On the old
        // implementation, ctx.os() was called from inside input_mut and
        // deadlocked on Windows as well as macOS.
        let ctx = egui::Context::default();
        let commands = consume(&ctx, false);
        assert!(commands.is_empty());
    }

    #[test]
    fn mac_backspace_alias_path_does_not_reenter_context_lock() {
        // Cover the macOS-only alias even when tests run on another OS.
        let ctx = egui::Context::default();
        let commands = consume_with_platform(&ctx, false, true);
        assert!(commands.is_empty());
    }
}
