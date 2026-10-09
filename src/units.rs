//! Unités de taille et de débit : octets (o), kilooctets (Ko), mégaoctets (Mo),
//! gigaoctets (Go) — base 1024 — et formatage lisible à la française.

/// Unité de saisie sélectionnable dans l'interface.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Unit {
    /// octets
    O,
    /// kilooctets (1 024 o)
    Ko,
    /// mégaoctets (1 024 Ko)
    Mo,
    /// gigaoctets (1 024 Mo)
    Go,
}

impl Unit {
    pub const ALL: [Unit; 4] = [Unit::O, Unit::Ko, Unit::Mo, Unit::Go];

    pub fn label(self) -> &'static str {
        match self {
            Unit::O => "o",
            Unit::Ko => "Ko",
            Unit::Mo => "Mo",
            Unit::Go => "Go",
        }
    }

    /// Facteur de conversion vers les octets.
    pub fn factor(self) -> u64 {
        const K: u64 = 1024;
        match self {
            Unit::O => 1,
            Unit::Ko => K,
            Unit::Mo => K * K,
            Unit::Go => K * K * K,
        }
    }
}

/// Convertit une valeur saisie + unité en octets/s.
/// Renvoie 0 si la valeur est nulle, vide ou négative (convention : 0 = illimité).
pub fn to_bytes_per_sec(value: f64, unit: Unit) -> u64 {
    if !value.is_finite() || value <= 0.0 {
        return 0;
    }
    let bytes = value * unit.factor() as f64;
    if bytes >= u64::MAX as f64 {
        u64::MAX
    } else {
        bytes as u64
    }
}

/// Formate un débit en octets/s : `1234.0` -> « 1,2 Ko/s », `0.0` -> « 0 o/s ».
pub fn format_speed(bytes_per_sec: f64) -> String {
    format_octets(bytes_per_sec, true)
}

/// Formate une quantité d'octets : « 1,2 Ko », « 3,0 Go »…
pub fn format_bytes(bytes: f64) -> String {
    format_octets(bytes, false)
}

fn format_octets(v: f64, with_per_sec: bool) -> String {
    let suffix = if with_per_sec { "/s" } else { "" };
    let v = v.max(0.0);
    let (val, unit) = if v < 1024.0 {
        (v, "o")
    } else if v < 1024.0 * 1024.0 {
        (v / 1024.0, "Ko")
    } else if v < 1024.0f64.powi(3) {
        (v / (1024.0 * 1024.0), "Mo")
    } else if v < 1024.0f64.powi(4) {
        (v / 1024.0f64.powi(3), "Go")
    } else {
        (v / 1024.0f64.powi(4), "To")
    };
    let s = if unit == "o" {
        format!("{} {}{}", v as u64, unit, suffix)
    } else if val >= 100.0 {
        format!("{:.0} {}{}", val, unit, suffix)
    } else {
        format!("{:.1} {}{}", val, unit, suffix)
    };
    // séparateur décimal français
    s.replace('.', ",")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn facteurs() {
        assert_eq!(Unit::O.factor(), 1);
        assert_eq!(Unit::Ko.factor(), 1024);
        assert_eq!(Unit::Mo.factor(), 1024 * 1024);
        assert_eq!(Unit::Go.factor(), 1024u64 * 1024 * 1024);
    }

    #[test]
    fn conversion_debits() {
        assert_eq!(to_bytes_per_sec(10.0, Unit::Mo), 10 * 1024 * 1024);
        assert_eq!(to_bytes_per_sec(1.5, Unit::Go), 1_610_612_736);
        assert_eq!(to_bytes_per_sec(512.0, Unit::Ko), 512 * 1024);
        assert_eq!(to_bytes_per_sec(2048.0, Unit::O), 2048);
        assert_eq!(to_bytes_per_sec(0.0, Unit::Mo), 0);
        assert_eq!(to_bytes_per_sec(-3.0, Unit::Mo), 0);
        assert_eq!(to_bytes_per_sec(f64::NAN, Unit::Mo), 0);
    }

    #[test]
    fn formatages() {
        assert_eq!(format_speed(0.0), "0 o/s");
        assert_eq!(format_speed(512.0), "512 o/s");
        assert_eq!(format_speed(1024.0), "1,0 Ko/s");
        assert_eq!(format_speed(10.0 * 1024.0 * 1024.0), "10,0 Mo/s");
        assert_eq!(format_speed(1.25 * 1024.0 * 1024.0 * 1024.0), "1,2 Go/s"); // arrondi half-to-even
        assert_eq!(format_bytes(3.0 * 1024.0 * 1024.0 * 1024.0), "3,0 Go");
        assert_eq!(format_bytes(250.0), "250 o");
    }

    #[test]
    fn etiquettes() {
        assert_eq!(Unit::O.label(), "o");
        assert_eq!(Unit::Go.label(), "Go");
    }
}
