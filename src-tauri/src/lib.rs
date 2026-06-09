use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_autostart::ManagerExt;
use tauri_plugin_clipboard_manager::ClipboardExt;
use tokio::sync::Mutex;

#[cfg(target_os = "windows")]
use winapi::um::winuser::{COLOR_WINDOW, GetSysColor};

mod config;
mod history;
mod provider_factory;
pub mod theme;
mod trans_azure;
mod trans_azure_translator;
mod trans_lm_studio;
mod trans_ollama;
mod trans_openai;
mod translation;
mod tray;

use config::Config;
use history::{
    TranslationHistory, add_translation_to_history, clear_translation_history, deduplicate_history,
    delete_history_entry, fix_target_language_in_history, get_translation_history,
};
use translation::{AlternativeTranslationsResult, TranslationResult, TranslationService};

// Application state
pub struct AppState {
    pub config: Arc<Mutex<Config>>,
    pub translation_service: Arc<Mutex<TranslationService>>,
}

#[cfg(target_os = "windows")]
fn is_dark_theme() -> bool {
    unsafe {
        // Get the background color of windows
        let color = GetSysColor(COLOR_WINDOW);
        let red = (color & 0xFF) as u8;
        let green = ((color >> 8) & 0xFF) as u8;
        let blue = ((color >> 16) & 0xFF) as u8;

        // Calculate luminance using the standard formula
        let luminance = (0.299 * red as f64) + (0.587 * green as f64) + (0.114 * blue as f64);

        // If luminance is low, it's likely a dark theme
        luminance < 128.0
    }
}

#[cfg(not(target_os = "windows"))]
fn is_dark_theme() -> bool {
    false // Default to light theme on non-Windows platforms
}

#[tauri::command]
async fn get_windows_theme() -> Result<String, String> {
    if is_dark_theme() {
        Ok("dark".to_string())
    } else {
        Ok("light".to_string())
    }
}

#[tauri::command]
async fn show_main_window(app: AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window("main") {
        window.show().map_err(|e| e.to_string())?;
        window.set_focus().map_err(|e| e.to_string())?;
        Ok(())
    } else {
        Err("Main window not found".to_string())
    }
}

#[tauri::command]
async fn get_clipboard_text(app: AppHandle) -> Result<String, String> {
    app.clipboard()
        .read_text()
        .map_err(|e| format!("Failed to read clipboard: {}", e))
}

// Removed translate_text function to prevent duplicate history entries
// Now using only the translate function which has better duplicate detection

#[tauri::command]
async fn translate(text: String, config: State<'_, AppState>) -> Result<TranslationResult, String> {
    match translation::translate_text(text, config).await {
        Ok(response) => {
            // Add to history
            if let Err(e) = add_translation_to_history(
                response.original_text.clone(),
                response.translated_text.clone(),
                response.detected_language.clone(),
                response.target_language.clone(),
            ) {
                log::error!("Failed to add translation to history: {}", e);
            } // Convert TranslationResponse to TranslationResult for return
            Ok(TranslationResult {
                detected_language: response.detected_language,
                translated_text: response.translated_text,
                target_language: response.target_language,
            })
        }
        Err(translation::Error::DuplicateRequest) => {
            // For duplicate requests, we'll just return an empty response
            // The UI will handle this appropriately
            log::info!("Skipping duplicate translation request");
            Err("Duplicate request".to_string())
        }
        Err(e) => Err(e.to_string()),
    }
}

#[tauri::command]
async fn get_config(state: State<'_, AppState>) -> Result<Config, String> {
    let config = state.config.lock().await;
    Ok(config.clone())
}

#[tauri::command]
async fn save_config(
    new_config: Config,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    // Check if hotkey changed
    let old_config = {
        let config = state.config.lock().await;
        config.clone()
    };

    let hotkey_changed = old_config.hotkey != new_config.hotkey;

    match new_config.save() {
        Ok(_) => {
            // Update the config in the state
            let mut config = state.config.lock().await;
            *config = new_config.clone();

            // Update translation service with new config
            let mut service = state.translation_service.lock().await;
            *service = TranslationService::new(new_config.clone());

            // Re-register global shortcut if hotkey changed
            if hotkey_changed && let Err(e) = setup_global_shortcut(&app, &new_config).await {
                log::error!("Failed to update global shortcut: {}", e);
            }

            Ok(())
        }
        Err(e) => Err(format!("Failed to save config: {}", e)),
    }
}

#[tauri::command]
async fn copy_to_clipboard(text: String, app: AppHandle) -> Result<(), String> {
    app.clipboard()
        .write_text(text)
        .map_err(|e| format!("Failed to copy to clipboard: {}", e))
}

#[tauri::command]
async fn test_translation_from_clipboard(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<TranslationResult, String> {
    // Get text from clipboard
    let text = app
        .clipboard()
        .read_text()
        .map_err(|e| format!("Failed to read clipboard: {}", e))?;

    if text.trim().is_empty() {
        return Err("Clipboard is empty".to_string());
    }

    // Translate the text
    let service = state.translation_service.lock().await;
    match service.detect_and_translate(&text).await {
        Ok(result) => {
            log::info!(
                "Translation test successful: {} -> {}",
                result.detected_language,
                result.translated_text
            );
            Ok(result)
        }
        Err(e) => {
            log::error!("Translation test failed: {}", e);
            Err(format!("Translation failed: {}", e))
        }
    }
}

#[tauri::command]
async fn get_alternative_translations(
    selected_text: String,
    target_language: String,
    state: State<'_, AppState>,
) -> Result<AlternativeTranslationsResult, String> {
    match translation::get_alternative_translations(selected_text, target_language, state).await {
        Ok(result) => Ok(result),
        Err(e) => Err(e.to_string()),
    }
}

#[tauri::command]
async fn get_alternative_translations_debug(
    selected_text: String,
    target_language: String,
    state: State<'_, AppState>,
) -> Result<serde_json::Value, String> {
    match translation::get_alternative_translations_debug(selected_text, target_language, state)
        .await
    {
        Ok(result) => Ok(result),
        Err(e) => Err(e.to_string()),
    }
}

fn extract_api_version_from_url(url: &str) -> Option<String> {
    // Try to extract api-version from URL query parameters
    if let Ok(parsed_url) = url::Url::parse(url) {
        for (key, value) in parsed_url.query_pairs() {
            if key == "api-version" {
                return Some(value.to_string());
            }
        }
    }
    None
}

#[tauri::command]
async fn validate_api_key(
    api_provider: String,
    api_key: String,
    endpoint: Option<String>,
    api_version: Option<String>,
    region: Option<String>,
) -> Result<bool, String> {
    let client = reqwest::Client::new();

    match api_provider.as_str() {
        "openai" => {
            let response = client
                .get("https://api.openai.com/v1/models")
                .header("Authorization", format!("Bearer {}", api_key))
                .send()
                .await
                .map_err(|e| format!("Request failed: {}", e))?;

            Ok(response.status().is_success())
        }
        "azure_openai" => {
            if let Some(endpoint) = endpoint {
                // Use provided api_version, or try to extract from endpoint, or use default
                let version = api_version
                    .or_else(|| extract_api_version_from_url(&endpoint))
                    .unwrap_or_else(|| "2025-01-01-preview".to_string());

                // Determine endpoint type based on hostname
                let is_models_endpoint = endpoint.contains("services.ai.azure.com");

                let url = if is_models_endpoint {
                    // Models API endpoint - use /models endpoint for validation
                    format!(
                        "{}/models?api-version={}",
                        endpoint.trim_end_matches('/'),
                        version
                    )
                } else {
                    // Cognitive Services endpoint - use /openai/models endpoint for validation
                    format!(
                        "{}/openai/models?api-version={}",
                        endpoint.trim_end_matches('/'),
                        version
                    )
                };

                let response = client
                    .get(&url)
                    .header("api-key", &api_key)
                    .send()
                    .await
                    .map_err(|e| format!("Request failed: {}", e))?;

                Ok(response.status().is_success())
            } else {
                Err("Azure endpoint is required".to_string())
            }
        }
        "ollama" => {
            if let Some(ollama_url) = endpoint {
                // Test connection to Ollama server by checking if it's running
                let url = format!("{}/api/tags", ollama_url.trim_end_matches('/'));

                let response = client
                    .get(&url)
                    .send()
                    .await
                    .map_err(|e| format!("Ollama server connection failed: {}", e))?;

                Ok(response.status().is_success())
            } else {
                Err("Ollama URL is required".to_string())
            }
        }
        "lm_studio" => {
            if let Some(lm_studio_url) = endpoint {
                // Test connection to LM Studio server via its OpenAI-compatible models endpoint
                let url = format!("{}/v1/models", lm_studio_url.trim_end_matches('/'));

                let response = client
                    .get(&url)
                    .send()
                    .await
                    .map_err(|e| format!("LM Studio server connection failed: {}", e))?;

                Ok(response.status().is_success())
            } else {
                Err("LM Studio URL is required".to_string())
            }
        }
        "azure_translator" => {
            if let Some(endpoint) = endpoint {
                // Test Azure Translator with a simple language detection call
                let url = format!("{}/detect?api-version=3.0", endpoint.trim_end_matches('/'));
                let test_body = serde_json::json!([{"Text": "Hello"}]);

                let mut request = client
                    .post(&url)
                    .header("Ocp-Apim-Subscription-Key", &api_key)
                    .header("Content-Type", "application/json; charset=UTF-8");

                // Add region header if provided
                if let Some(region) = region
                    && !region.is_empty()
                {
                    request = request.header("Ocp-Apim-Subscription-Region", &region);
                }

                let response = request
                    .json(&test_body)
                    .send()
                    .await
                    .map_err(|e| format!("Azure Translator API request failed: {}", e))?;

                Ok(response.status().is_success())
            } else {
                Err("Azure Translator endpoint is required".to_string())
            }
        }
        _ => Err("Unsupported API provider".to_string()),
    }
}

/// Fetch the list of loaded model ids from an LM Studio server (OpenAI-compatible GET /v1/models).
#[tauri::command]
async fn fetch_lm_studio_models(url: String) -> Result<Vec<String>, String> {
    let endpoint = if url.trim().is_empty() {
        "http://127.0.0.1:1234".to_string()
    } else {
        url.trim().to_string()
    };
    let models_url = format!("{}/v1/models", endpoint.trim_end_matches('/'));

    let client = reqwest::Client::new();
    let response = client
        .get(&models_url)
        .send()
        .await
        .map_err(|e| format!("Failed to reach LM Studio server: {}", e))?;

    if !response.status().is_success() {
        return Err(format!(
            "LM Studio server returned status {}",
            response.status()
        ));
    }

    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse LM Studio models response: {}", e))?;

    let models = body
        .get("data")
        .and_then(|d| d.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|m| m.get("id").and_then(|id| id.as_str()).map(String::from))
                .collect::<Vec<String>>()
        })
        .unwrap_or_default();

    Ok(models)
}

#[tauri::command]
async fn get_translation_history_cmd() -> Result<TranslationHistory, String> {
    get_translation_history().map_err(|e| format!("Failed to get translation history: {}", e))
}

#[tauri::command]
async fn clear_translation_history_cmd() -> Result<(), String> {
    clear_translation_history().map_err(|e| format!("Failed to clear translation history: {}", e))
}

#[tauri::command]
async fn deduplicate_history_cmd() -> Result<(), String> {
    deduplicate_history().map_err(|e| format!("Failed to deduplicate translation history: {}", e))
}

#[tauri::command]
async fn delete_history_entry_cmd(entry_id: String) -> Result<(), String> {
    delete_history_entry(entry_id).map_err(|e| format!("Failed to delete history entry: {}", e))
}

#[tauri::command]
async fn fix_target_language_in_history_cmd() -> Result<(), String> {
    fix_target_language_in_history()
        .map_err(|e| format!("Failed to fix target language in history: {}", e))
}

#[tauri::command]
async fn reset_detected_language() -> Result<(), String> {
    log::info!("Detected language reset requested");
    Ok(())
}

fn parse_hotkey(hotkey: &str) -> Option<tauri_plugin_global_shortcut::Shortcut> {
    use tauri_plugin_global_shortcut::{Code, Modifiers, Shortcut};

    let parts: Vec<&str> = hotkey.split('+').map(|s| s.trim()).collect();
    if parts.is_empty() {
        return None;
    }

    let mut modifiers = Modifiers::empty();
    let mut code = None;

    for (i, part) in parts.iter().enumerate() {
        // The last part should be the key code
        if i == parts.len() - 1 {
            if part.len() == 1 {
                // Single character key
                let c = part.chars().next().unwrap().to_uppercase().next().unwrap();
                code = match c {
                    'A' => Some(Code::KeyA),
                    'B' => Some(Code::KeyB),
                    'C' => Some(Code::KeyC),
                    'D' => Some(Code::KeyD),
                    'E' => Some(Code::KeyE),
                    'F' => Some(Code::KeyF),
                    'G' => Some(Code::KeyG),
                    'H' => Some(Code::KeyH),
                    'I' => Some(Code::KeyI),
                    'J' => Some(Code::KeyJ),
                    'K' => Some(Code::KeyK),
                    'L' => Some(Code::KeyL),
                    'M' => Some(Code::KeyM),
                    'N' => Some(Code::KeyN),
                    'O' => Some(Code::KeyO),
                    'P' => Some(Code::KeyP),
                    'Q' => Some(Code::KeyQ),
                    'R' => Some(Code::KeyR),
                    'S' => Some(Code::KeyS),
                    'T' => Some(Code::KeyT),
                    'U' => Some(Code::KeyU),
                    'V' => Some(Code::KeyV),
                    'W' => Some(Code::KeyW),
                    'X' => Some(Code::KeyX),
                    'Y' => Some(Code::KeyY),
                    'Z' => Some(Code::KeyZ),
                    '0' => Some(Code::Digit0),
                    '1' => Some(Code::Digit1),
                    '2' => Some(Code::Digit2),
                    '3' => Some(Code::Digit3),
                    '4' => Some(Code::Digit4),
                    '5' => Some(Code::Digit5),
                    '6' => Some(Code::Digit6),
                    '7' => Some(Code::Digit7),
                    '8' => Some(Code::Digit8),
                    '9' => Some(Code::Digit9),
                    _ => None,
                };
            } else {
                // Special keys or function keys
                code = match part.to_lowercase().as_str() {
                    "f1" => Some(Code::F1),
                    "f2" => Some(Code::F2),
                    "f3" => Some(Code::F3),
                    "f4" => Some(Code::F4),
                    "f5" => Some(Code::F5),
                    "f6" => Some(Code::F6),
                    "f7" => Some(Code::F7),
                    "f8" => Some(Code::F8),
                    "f9" => Some(Code::F9),
                    "f10" => Some(Code::F10),
                    "f11" => Some(Code::F11),
                    "f12" => Some(Code::F12),
                    "space" => Some(Code::Space),
                    "tab" => Some(Code::Tab),
                    "escape" => Some(Code::Escape),
                    "enter" => Some(Code::Enter),
                    "backspace" => Some(Code::Backspace),
                    "insert" => Some(Code::Insert),
                    "delete" => Some(Code::Delete),
                    "home" => Some(Code::Home),
                    "end" => Some(Code::End),
                    "pageup" => Some(Code::PageUp),
                    "pagedown" => Some(Code::PageDown),
                    "left" => Some(Code::ArrowLeft),
                    "right" => Some(Code::ArrowRight),
                    "up" => Some(Code::ArrowUp),
                    "down" => Some(Code::ArrowDown),
                    _ => None,
                };
            }
        } else {
            // This part should be a modifier
            match part.to_lowercase().as_str() {
                "ctrl" | "control" | "commandorcontrol" => {
                    modifiers |= Modifiers::CONTROL;
                }
                "alt" | "option" => {
                    modifiers |= Modifiers::ALT;
                }
                "shift" => {
                    modifiers |= Modifiers::SHIFT;
                }
                "super" | "command" | "cmd" | "meta" => {
                    modifiers |= Modifiers::SUPER;
                }
                _ => {
                    log::warn!("Unknown modifier: {}", part);
                    return None;
                }
            }
        }
    }

    code.map(|code| Shortcut::new(Some(modifiers), code))
}

async fn setup_global_shortcut(
    app: &AppHandle,
    config: &Config,
) -> Result<(), Box<dyn std::error::Error>> {
    use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut};

    // Parse the hotkey from config
    let shortcut = parse_hotkey(&config.hotkey).unwrap_or_else(|| {
        log::warn!(
            "Invalid hotkey format: {}, using default Ctrl+Alt+C",
            config.hotkey
        );
        Shortcut::new(Some(Modifiers::CONTROL | Modifiers::ALT), Code::KeyC)
    }); // Unregister any existing shortcuts
    if let Err(e) = app.global_shortcut().unregister_all() {
        log::warn!("Failed to unregister existing shortcuts: {}", e);
    }

    let app_handle = app.clone();
    let hotkey_str = config.hotkey.clone();
    app.global_shortcut()
        .on_shortcut(shortcut, move |_app_handle, _shortcut, event| {
            log::info!(
                "Global shortcut triggered: {} - Event: {:?}",
                hotkey_str,
                event
            );
            let app_clone = app_handle.clone();
            tauri::async_runtime::spawn(async move {
                handle_shortcut_activation(app_clone).await;
            });
        })?;

    log::info!("Global shortcut registered: {}", config.hotkey);
    Ok(())
}

/// Simulate a Ctrl+C keystroke so the foreground application copies its current
/// selection to the clipboard. Windows-only; a no-op on other platforms.
#[cfg(target_os = "windows")]
fn simulate_copy() {
    use std::mem::{size_of, zeroed};
    use winapi::um::winuser::{
        INPUT, INPUT_KEYBOARD, KEYEVENTF_KEYUP, SendInput, VK_CONTROL,
    };

    const VK_C: u16 = 0x43;

    fn key_event(vk: u16, key_up: bool) -> INPUT {
        // SAFETY: zero-initializing a POD INPUT struct and writing its keyboard union.
        unsafe {
            let mut input: INPUT = zeroed();
            input.type_ = INPUT_KEYBOARD;
            let ki = input.u.ki_mut();
            ki.wVk = vk;
            ki.dwFlags = if key_up { KEYEVENTF_KEYUP } else { 0 };
            input
        }
    }

    let mut inputs = [
        key_event(VK_CONTROL as u16, false),
        key_event(VK_C, false),
        key_event(VK_C, true),
        key_event(VK_CONTROL as u16, true),
    ];

    // SAFETY: `inputs` is a valid, non-empty array of INPUT structs.
    let sent = unsafe {
        SendInput(
            inputs.len() as u32,
            inputs.as_mut_ptr(),
            size_of::<INPUT>() as i32,
        )
    };
    if sent != inputs.len() as u32 {
        log::warn!("SendInput sent {} of {} events", sent, inputs.len());
    }
}

#[cfg(not(target_os = "windows"))]
fn simulate_copy() {
    // Selection capture via simulated copy is only implemented on Windows.
}

/// Current mouse cursor position in physical screen pixels (Windows-only).
#[cfg(target_os = "windows")]
fn get_cursor_position() -> Option<(i32, i32)> {
    use winapi::shared::windef::POINT;
    use winapi::um::winuser::GetCursorPos;

    unsafe {
        let mut point = POINT { x: 0, y: 0 };
        if GetCursorPos(&mut point) != 0 {
            Some((point.x, point.y))
        } else {
            None
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn get_cursor_position() -> Option<(i32, i32)> {
    None
}

/// Position the floating window near the cursor (clamped to the cursor's monitor) and show it.
fn position_and_show_floating(app: &AppHandle) -> tauri::Result<()> {
    let Some(window) = app.get_webview_window("floating") else {
        log::error!("Floating window not found");
        return Ok(());
    };

    if let Some((cx, cy)) = get_cursor_position() {
        // Default: slightly below-right of the cursor.
        let mut x = cx + 16;
        let mut y = cy + 16;

        // Best-effort clamp to the monitor under the cursor so the popup stays on-screen.
        if let Ok(monitors) = window.available_monitors()
            && let Some(monitor) = monitors.into_iter().find(|m| {
                let pos = m.position();
                let size = m.size();
                cx >= pos.x
                    && cx < pos.x + size.width as i32
                    && cy >= pos.y
                    && cy < pos.y + size.height as i32
            })
        {
            let mpos = monitor.position();
            let msize = monitor.size();
            let scale = monitor.scale_factor();
            let win_w = (380.0 * scale) as i32;
            let win_h = (240.0 * scale) as i32;
            let max_x = mpos.x + msize.width as i32 - win_w;
            let max_y = mpos.y + msize.height as i32 - win_h;
            x = x.clamp(mpos.x, max_x.max(mpos.x));
            y = y.clamp(mpos.y, max_y.max(mpos.y));
        }

        let _ = window.set_position(tauri::PhysicalPosition::new(x, y));
    }

    window.show()?;
    window.set_focus()?;
    Ok(())
}

#[tauri::command]
async fn show_floating_at_cursor(app: AppHandle) -> Result<(), String> {
    position_and_show_floating(&app).map_err(|e| e.to_string())
}

#[tauri::command]
async fn set_target_language(language: String, state: State<'_, AppState>) -> Result<(), String> {
    let updated = {
        let mut config = state.config.lock().await;
        config.target_language = language;
        config.save().map_err(|e| format!("Failed to save config: {}", e))?;
        config.clone()
    };
    let mut service = state.translation_service.lock().await;
    *service = TranslationService::new(updated);
    Ok(())
}

#[tauri::command]
async fn hide_floating_window(app: AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window("floating") {
        window.hide().map_err(|e| e.to_string())?;
    }
    Ok(())
}

async fn handle_shortcut_activation(app: AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        // Always reset detected language first, regardless of focus state
        let _ = window.emit("reset-detected-language", ());
        log::info!("Global shortcut triggered - resetting detected language");

        // Whether the unfocused hotkey should drive the selection -> floating popup flow.
        let floating_on_hotkey = {
            let state = app.state::<AppState>();
            let config = state.config.lock().await;
            config.floating_on_hotkey
        };

        // Check if window is focused to determine additional behavior
        match window.is_focused() {
            Ok(is_focused) => {
                if !is_focused {
                    // Window is not focused - capture from the foreground app
                    if floating_on_hotkey {
                        handle_selection_capture(&app).await;
                    } else {
                        handle_clipboard_capture(&app, &window).await;
                    }
                }
                // If focused, only reset detected language (already done above)
            }
            Err(e) => {
                log::error!("Failed to check window focus state: {}", e);
                // Fallback to capture behavior
                if floating_on_hotkey {
                    handle_selection_capture(&app).await;
                } else {
                    handle_clipboard_capture(&app, &window).await;
                }
            }
        }
    }
}

/// Simulate a copy of the foreground selection, then show the captured text in the
/// floating popup for translation.
async fn handle_selection_capture(app: &AppHandle) {
    // Copy the current selection from the foreground app into the clipboard.
    simulate_copy();
    // Give the foreground app time to populate the clipboard.
    tokio::time::sleep(tokio::time::Duration::from_millis(120)).await;

    match app.clipboard().read_text() {
        Ok(text) if !text.trim().is_empty() => {
            if let Err(e) = position_and_show_floating(app) {
                log::error!("Failed to show floating window: {}", e);
            }
            if let Some(floating) = app.get_webview_window("floating") {
                let _ = floating.emit("selection-text", &text);
                log::info!("Selection text sent to floating window");
            }
        }
        Ok(_) => {
            log::warn!("Selection capture: clipboard empty after simulated copy");
        }
        Err(e) => {
            log::error!("Selection capture: failed to read clipboard: {}", e);
        }
    }
}

async fn handle_clipboard_capture(app: &AppHandle, window: &tauri::WebviewWindow) {
    // Add a small delay to ensure clipboard is updated
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    match app.clipboard().read_text() {
        Ok(text) => {
            if !text.trim().is_empty() {
                // Show window
                let _ = window.show();
                let _ = window.set_focus();

                // Emit clipboard text event to frontend
                let _ = window.emit("clipboard-text", &text);
                log::info!("Clipboard text sent to frontend: {}", text);
            } else {
                log::warn!("Clipboard is empty");
                // Still show the window even if clipboard is empty
                let _ = window.show();
                let _ = window.set_focus();
            }
        }
        Err(e) => {
            log::error!("Failed to read clipboard: {}", e);
            // Still show the window even if clipboard reading fails
            let _ = window.show();
            let _ = window.set_focus();
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Initialize logging
    env_logger::init();

    let config = Config::load().unwrap_or_else(|e| {
        log::warn!("Failed to load config, using default: {}", e);
        Config::default()
    });

    let translation_service = TranslationService::new(config.clone());
    let app_state = AppState {
        config: Arc::new(Mutex::new(config.clone())),
        translation_service: Arc::new(Mutex::new(translation_service)),
    };

    let mut builder = tauri::Builder::default();

    builder = builder
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--autostart"]),
        ))
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.show();
                let _ = window.set_focus();
            }
        }));

    builder
        .manage(app_state)
        .on_window_event(|window, event| {
            match event {
                tauri::WindowEvent::CloseRequested { api, .. } => {
                    // Get the config to check minimize_to_tray setting
                    let app_state = window.state::<AppState>();
                    let config = app_state.config.blocking_lock();

                    if config.minimize_to_tray {
                        // Prevent the default close behavior
                        api.prevent_close();
                        // Hide the window instead
                        let _ = window.hide();
                        log::info!("Window hidden to tray instead of closed");
                    } else {
                        // Allow normal close behavior
                        log::info!("Window closed normally");
                    }
                }
                tauri::WindowEvent::Resized(..) => {
                    // Check if window is minimized
                    if let Ok(is_minimized) = window.is_minimized()
                        && is_minimized
                    {
                        // Get the config to check minimize_to_tray setting
                        let app_state = window.state::<AppState>();
                        let config = app_state.config.blocking_lock();

                        if config.minimize_to_tray {
                            // Hide the window when minimized
                            let _ = window.hide();
                            log::info!("Window hidden to tray on minimize");
                        }
                    }
                }
                _ => {}
            }
        })
        .setup(move |app| {
            // Create system tray
            if let Err(e) = tray::create_tray(app.handle()) {
                log::error!("Failed to create tray: {}", e);
            } // Setup global shortcut
            let config_clone = config.clone();
            let app_handle = app.handle().clone();

            tauri::async_runtime::spawn(async move {
                if let Err(e) = setup_global_shortcut(&app_handle, &config_clone).await {
                    log::error!("Failed to setup global shortcut: {}", e);
                }
            });

            // Setup autostart if enabled
            if config.auto_start {
                let autostart = app.autolaunch();
                if let Err(e) = autostart.enable() {
                    log::error!("Failed to enable autostart: {}", e);
                }
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            show_main_window,
            show_floating_at_cursor,
            set_target_language,
            hide_floating_window,
            get_clipboard_text,
            translate,
            get_config,
            save_config,
            copy_to_clipboard,
            test_translation_from_clipboard,
            get_windows_theme,
            validate_api_key,
            fetch_lm_studio_models,
            get_translation_history_cmd,
            clear_translation_history_cmd,
            deduplicate_history_cmd,
            delete_history_entry_cmd,
            fix_target_language_in_history_cmd,
            reset_detected_language,
            get_alternative_translations,
            get_alternative_translations_debug
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
