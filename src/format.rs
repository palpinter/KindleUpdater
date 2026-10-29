/// Formats a byte count with decimal units and a Hungarian decimal comma, e.g. `17,4 MB`.
pub fn size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["kB", "MB", "GB", "TB"];
    if bytes < 1000 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1000.0;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    let text = if value >= 100.0 {
        format!("{value:.0}")
    } else {
        format!("{value:.1}").replace('.', ",")
    };
    format!("{text} {}", UNITS[unit])
}

#[cfg(test)]
mod tests {
    use super::size;

    #[test]
    fn formats_sizes() {
        assert_eq!(size(0), "0 B");
        assert_eq!(size(999), "999 B");
        assert_eq!(size(1_000), "1,0 kB");
        assert_eq!(size(308_178), "308 kB");
        assert_eq!(size(17_412_345), "17,4 MB");
        assert_eq!(size(1_100_000_000), "1,1 GB");
    }
}
