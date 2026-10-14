use leptos::*;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

#[component]
pub fn App() -> impl IntoView {
    let (status, set_status) = create_signal("Connecting...".to_string());
    let (status_color, set_status_color) = create_signal("#eab308".to_string());

    create_effect(move |_| {
        let window = web_sys::window().unwrap();
        let closure = Closure::<dyn FnMut(web_sys::CustomEvent)>::wrap(Box::new(move |ev: web_sys::CustomEvent| {
            if let Ok(detail) = js_sys::Reflect::get(&ev.detail(), &JsValue::from_str("text")) {
                if let Some(txt) = detail.as_string() {
                    set_status.set(txt);
                }
            }
            if let Ok(detail) = js_sys::Reflect::get(&ev.detail(), &JsValue::from_str("color")) {
                if let Some(col) = detail.as_string() {
                    set_status_color.set(col);
                }
            }
        }));

        let _ = window.add_event_listener_with_callback("shell-status", closure.as_ref().unchecked_ref());
        closure.forget();

        let _ = js_sys::eval("setTimeout(() => window.mountXterm('terminal-container'), 50);");
    });

    let restart_session = move |_| {
        let _ = js_sys::eval("if (window.__shellWs) { window.__shellWs.close(); } window.mountXterm('terminal-container');");
    };

    view! {
        <div style="display: flex; flex-direction: column; height: 100vh; width: 100vw; background-color: #0f172a; color: #f8fafc; font-family: 'JetBrains Mono', 'Fira Code', 'Courier New', monospace; overflow: hidden;">
            <div style="display: flex; align-items: center; justify-content: space-between; padding: 10px 18px; background-color: #1e293b; border-bottom: 1px solid #334155; flex-shrink: 0;">
                <div style="display: flex; align-items: center; gap: 12px;">
                    <div style="display: flex; gap: 6px;">
                        <span style="width: 12px; height: 12px; border-radius: 50%; background-color: #ef4444; display: inline-block;"></span>
                        <span style="width: 12px; height: 12px; border-radius: 50%; background-color: #f59e0b; display: inline-block;"></span>
                        <span style="width: 12px; height: 12px; border-radius: 50%; background-color: #10b981; display: inline-block;"></span>
                    </div>
                    <span style="font-weight: 600; font-size: 14px; letter-spacing: 0.5px; color: #38bdf8;">
                        "🦀 Rust Online Shell (WASM + Axum)"
                    </span>
                </div>

                <div style="display: flex; align-items: center; gap: 16px;">
                    <div style="display: flex; align-items: center; gap: 8px; font-size: 13px;">
                        <span style=move || format!("width: 8px; height: 8px; border-radius: 50%; background-color: {}; display: inline-block;", status_color.get())></span>
                        <span style="color: #94a3b8;">{status}</span>
                    </div>
                    <button
                        on:click=restart_session
                        style="background-color: #334155; color: #f1f5f9; border: 1px solid #475569; border-radius: 6px; padding: 5px 12px; font-size: 12px; font-weight: 500; cursor: pointer; transition: all 0.2s;"
                    >
                        "↻ Restart Session"
                    </button>
                </div>
            </div>

            <div style="display: flex; gap: 16px; padding: 6px 18px; background-color: #090d16; border-bottom: 1px solid #1e293b; font-size: 12px; color: #64748b; flex-shrink: 0;">
                <span>"💡 Supports Tab Completion"</span>
                <span>"•"</span>
                <span>"Up/Down Arrow History"</span>
                <span>"•"</span>
                <span>"Pipes (|) & Redirections (>, >>)"</span>
                <span>"•"</span>
                <span>"Persistent User Sandbox"</span>
            </div>

            <div
                id="terminal-container"
                style="flex: 1; width: 100%; height: 100%; padding: 12px 16px; background-color: #0f172a; overflow: hidden;"
            >
            </div>
        </div>
    }
}

fn main() {
    console_error_panic_hook::set_once();
    mount_to_body(|| view! { <App/> });
}
