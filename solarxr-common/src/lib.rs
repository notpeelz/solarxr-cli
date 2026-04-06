// Cargo enforces semver, so have to resort to this hack to
// strip the major version component.
const fn format_version(s: &str) -> &str {
    assert!(s.is_ascii());
    let mut bytes = s.as_bytes();
    let mut dot_count = 0;
    loop {
        match bytes {
            [] => panic!("invalid version string"),
            [b'.', remaining @ ..] => {
                dot_count += 1;
                bytes = remaining;
            }
            [..] if dot_count == 1 => {
                break;
            }
            [_, remaining @ ..] => {
                bytes = remaining;
            }
        }
    }
    unsafe { str::from_utf8_unchecked(bytes) }
}

pub const VERSION: &str = format_version(env!("CARGO_PKG_VERSION"));
