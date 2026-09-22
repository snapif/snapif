use snapif::Decision;

fn main() {
    let decision = Decision::Known(1_u8);
    match decision {
        Decision::Known(value) => {
            let _ = value;
        }
        _ => panic!("wildcard"),
    }
}
