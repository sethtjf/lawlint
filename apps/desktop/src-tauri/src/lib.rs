use lawlint_core::{lint as lint_text, LintOptions, LintResult};

#[tauri::command]
fn lint(text: String, options: Option<LintOptions>) -> LintResult {
    lint_text(&text, &options.unwrap_or_default())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .invoke_handler(tauri::generate_handler![lint])
        .run(tauri::generate_context!())
        .expect("error while running lawlint desktop application");
}
