use folio_spike::events::BridgeEvent;
use proptest::prelude::*;

#[test]
fn tap_json_from_js_shape() {
    let raw = r#"{"type":"tap","x":100.0,"y":200.5}"#;
    let ev: BridgeEvent = serde_json::from_str(raw).unwrap();
    assert_eq!(ev, BridgeEvent::Tap { x: 100.0, y: 200.5 });
}

proptest! {
    #[test]
    fn tap_roundtrips(x in -1000i32..8000, y in -1000i32..8000) {
        let ev = BridgeEvent::Tap {
            x: f64::from(x),
            y: f64::from(y),
        };
        let json = serde_json::to_string(&ev).unwrap();
        let back: BridgeEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(ev, back);
    }
}
