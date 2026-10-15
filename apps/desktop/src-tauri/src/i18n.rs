// SPDX-License-Identifier: GPL-3.0-or-later
//! The words this side shows (the tray, the About box, window titles) in the language the
//! window speaks, from the saved settings. The page has its own (`lib/i18n.tsx`).

use geserdesk_app::Language;
use tauri::{AppHandle, Manager};

use crate::state::AppState;

/// `id` or `en`, by the saved language.
pub fn tr(app: &AppHandle, id: &'static str, en: &'static str) -> &'static str {
    match app.state::<AppState>().settings().language {
        Language::Id => id,
        Language::En => en,
    }
}

/// Whether the saved language is English (for text built with `format!`).
pub fn english(app: &AppHandle) -> bool {
    app.state::<AppState>().settings().language == Language::En
}
