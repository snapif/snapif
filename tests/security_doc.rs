#[test]
fn security_md_names_the_supported_line() {
    let text = include_str!("../SECURITY.md");
    let section = text
        .split("## Supported versions")
        .nth(1)
        .expect("supported versions section");
    let section = section.split("## ").next().expect("section body");
    assert!(
        section.contains("| 0.2.x | yes |"),
        "0.2.x is the line that receives fixes:\n{section}"
    );
    assert!(
        section.contains("| 0.1.x | no |"),
        "0.1.x does not receive fixes:\n{section}"
    );
    assert!(
        !section.contains("| 0.1.x | yes |"),
        "the old supported row must not remain:\n{section}"
    );
}
