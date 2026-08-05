//! Making startup failures visible.
//!
//! `eprintln!` goes nowhere in a browser, so a client that cannot open a GPU
//! device used to leave a black page and an empty console. Failures land in
//! both the console and the page itself instead.

/// Reports a fatal startup failure to the console and to the page.
pub fn report_startup_failure(message: &str) {
    let text = format!("Treeline could not start: {message}");
    web_sys::console::error_1(&text.as_str().into());

    let Some(body) = web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| document.body())
    else {
        return;
    };
    let Ok(banner) = body.owner_document().map_or_else(
        || Err(wasm_bindgen::JsValue::NULL),
        |document| document.create_element("p"),
    ) else {
        return;
    };
    banner.set_text_content(Some(&text));
    let _ = banner.set_attribute(
        "style",
        "position:fixed;z-index:9;inset:auto 1rem 1rem 1rem;padding:1rem;\
         border-radius:0.4rem;background:rgb(9 17 13 / 92%);text-align:center",
    );
    let _ = body.append_child(&banner);
}
