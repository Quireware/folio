// Rust guideline compliant 2026-09-09
pub mod events;
pub mod server;

use dioxus::prelude::*;
use events::BridgeEvent;

const MAX_BRIDGE_COORDINATE: f64 = 100_000.0;
const PAINT_BACKGROUND_SCRIPT: &str = r#"
document.body.style.backgroundColor = "rgb(0, 128, 0)";
dioxus.send(document.body.style.backgroundColor === "rgb(0, 128, 0)");
await dioxus.recv();
"#;

pub fn app() -> Element {
    let last_tap = use_signal(|| Option::<BridgeEvent>::None);
    let mut count = use_signal(|| 0_u32);
    let image_url = use_resource(server::bind_and_serve);
    use_tap_bridge(last_tap);

    let server_status = render_server_status(image_url);
    let tap_log = render_tap_log(last_tap);

    rsx! {
        main {
            h1 { "folio spike" }
            {server_status}
            {tap_log}
            p { id: "count", "count: {count}" }
            button {
                id: "inc",
                onclick: move |_| count.with_mut(|value| *value = value.saturating_add(1)),
                "increment"
            }
            button {
                id: "paint",
                onclick: move |_| paint_background(),
                "paint background"
            }
        }
    }
}

fn render_server_status(image_url: Resource<Result<url::Url, server::ServerError>>) -> Element {
    match &*image_url.read_unchecked() {
        Some(Ok(url)) => rsx! {
            img {
                id: "test-image",
                src: url.as_str(),
                alt: "test image",
                width: "64",
                height: "64",
            }
            p { id: "server-status", "image URL: {url}" }
        },
        Some(Err(error)) => rsx! {
            p { id: "server-status", "local server failed: {error}" }
        },
        None => rsx! {
            p { id: "server-status", "starting local server" }
        },
    }
}

fn render_tap_log(last_tap: Signal<Option<BridgeEvent>>) -> Element {
    match &*last_tap.read() {
        Some(event) => rsx! { p { id: "tap-log", "last tap: {event:?}" } },
        None => rsx! { p { id: "tap-log", "last tap: none" } },
    }
}

fn use_tap_bridge(mut last_tap: Signal<Option<BridgeEvent>>) {
    use_effect(move || {
        spawn(async move {
            let mut eval = document::eval(include_str!("../assets/bridge.js"));
            loop {
                match eval.recv::<BridgeEvent>().await {
                    Ok(event) if bridge_event_is_valid(&event) => {
                        eprintln!("folio-spike tap: {event:?}");
                        last_tap.set(Some(event));
                    }
                    Ok(event) => eprintln!("folio-spike rejected tap: {event:?}"),
                    Err(error) => {
                        eprintln!("folio-spike bridge stopped: {error}");
                        break;
                    }
                }
            }
        });
    });
}

fn bridge_event_is_valid(event: &BridgeEvent) -> bool {
    let BridgeEvent::Tap { x, y } = event;
    [x, y]
        .into_iter()
        .all(|coordinate| (0.0..=MAX_BRIDGE_COORDINATE).contains(coordinate))
}

async fn paint_background() {
    let mut eval = document::eval(PAINT_BACKGROUND_SCRIPT);
    match eval.recv::<bool>().await {
        Ok(painted) => {
            if let Err(error) = eval.send(()) {
                eprintln!("folio-spike paint cleanup failed: {error}");
            } else if !painted {
                eprintln!("folio-spike paint result did not match");
            }
        }
        Err(error) => eprintln!("folio-spike paint failed: {error}"),
    }
}
