//! Maths that comes out the same on every machine.
//!
//! The sim must give the same result, bit for bit, on Windows, macOS and
//! Linux: replays, shift codes and co-op play all rebuild a shift from its
//! commands. `+ - * /` and `sqrt` are exact IEEE operations, but `f32::sin`
//! and the rest call each platform's maths library, which can differ in the
//! last bit. These use `libm`, the same code everywhere.
//!
//! A test (`no_platform_maths_in_the_sim`) keeps the platform's versions
//! out of `src/sim`.

pub fn sin(x: f32) -> f32 {
    libm::sinf(x)
}

pub fn cos(x: f32) -> f32 {
    libm::cosf(x)
}

pub fn tan(x: f32) -> f32 {
    libm::tanf(x)
}

pub fn asin(x: f32) -> f32 {
    libm::asinf(x)
}

/// The angle of (x, y), as `y.atan2(x)` would give it.
pub fn atan2(y: f32, x: f32) -> f32 {
    libm::atan2f(y, x)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Calls a platform's maths library makes, which can differ between
    /// machines: written `.name(` after a value.
    const PLATFORM: [&str; 22] = [
        ".sin(", ".cos(", ".tan(", ".asin(", ".acos(", ".atan(", ".atan2(", ".sin_cos(", ".sinh(", ".cosh(",
        ".tanh(", ".asinh(", ".acosh(", ".atanh(", ".exp(", ".exp2(", ".exp_m1(", ".ln(", ".log(", ".log2(",
        ".log10(", ".powf(",
    ];

    #[test]
    fn no_platform_maths_in_the_sim() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/sim");
        let mut found = Vec::new();
        for entry in std::fs::read_dir(&dir).expect("src/sim") {
            let path = entry.expect("an entry").path();
            if path.extension().is_none_or(|e| e != "rs") || path.ends_with("math.rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).expect("a source file");
            for (n, line) in text.lines().enumerate() {
                let code = line.split("//").next().unwrap_or("");
                for call in PLATFORM.iter().filter(|c| code.contains(*c)) {
                    found.push(format!("{}:{}: {call}", path.file_name().unwrap().to_string_lossy(), n + 1));
                }
            }
        }
        assert!(found.is_empty(), "use sim::math instead of the platform's maths library: {found:#?}");
    }

    #[test]
    fn agrees_with_the_platform_closely() {
        for i in -50..50 {
            let x = i as f32 * 0.137;
            assert!((sin(x) - x.sin()).abs() < 1e-6);
            assert!((cos(x) - x.cos()).abs() < 1e-6);
            assert!((atan2(x, 1.3) - x.atan2(1.3)).abs() < 1e-6);
        }
        assert!((asin(0.5) - 0.5f32.asin()).abs() < 1e-6);
        assert!((tan(0.3) - 0.3f32.tan()).abs() < 1e-6);
    }
}
