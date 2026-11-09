use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum BridgeEvent {
    #[serde(rename = "tap")]
    Tap { x: f64, y: f64 },
}
