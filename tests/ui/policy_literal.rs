use snapif::ids::BatteryId;
use snapif::policy::{ChoiceGates, NoulPolicy, Policy, Signal};
use snapif::Fail;

fn main() {
    // review_below is under escalate_below. The literal must not compile:
    // `sealed` is private and is set only after `finish`.
    let _policy = Policy {
        schema_version: 1,
        fail: Fail::Closed,
        shadow: false,
        cascade_min: 0.8,
        choice: ChoiceGates {
            escalate_below: 0.8,
            review_below: 0.1,
            signal: Signal::Confidence,
        },
        noul: NoulPolicy {
            yes_auto: 0.9,
            no_auto: 0.1,
        },
        default_action: None,
        actions: Default::default(),
        battery: BatteryId::new("tool-gate"),
    };
}
