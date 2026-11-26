fn bytes(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect()
}

fn check(golden: &str) -> (usize, Vec<String>) {
    let mut checked = 0;
    let mut failures = Vec::new();
    for line in golden
        .lines()
        .filter(|line| !line.starts_with('#') && !line.trim().is_empty())
    {
        let mut fields = line.splitn(3, '\t');
        let (wave, hex, expected) = (
            fields.next().unwrap(),
            fields.next().unwrap(),
            fields.next().unwrap(),
        );
        let code = bytes(hex);
        checked += 1;
        match rdna::decode_one(&code, 0, wave == "w32") {
            Ok(inst) if inst.bytes.len() != code.len() => {
                failures.push(format!(
                    "{hex}: consumed {} of {} bytes",
                    inst.bytes.len(),
                    code.len()
                ));
            }
            Ok(inst) if inst.text() != expected => {
                failures.push(format!(
                    "{hex}:\n  expected {expected}\n  decoded  {}",
                    inst.text()
                ));
            }
            Ok(_) => {}
            Err(_) => failures.push(format!("{hex}: invalid, expected {expected}")),
        }
    }
    (checked, failures)
}

#[test]
fn matches_llvm_mc_listings() {
    let (checked, failures) = check(include_str!("golden/llvm_mc.txt"));
    assert!(checked >= 40);
    assert!(
        failures.is_empty(),
        "{} of {checked} differ:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
