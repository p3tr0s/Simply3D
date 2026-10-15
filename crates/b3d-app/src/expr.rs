//! Tiny calculator for number fields: `20+5`, `(10+2)*3`, `1in`, `2.5 cm`, `12,5`,
//! `45°`, `0.5rad`, plus relative edits against the current value: `*2`, `/4`, `50%`.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// Result in millimetres.
    Length,
    /// Result in degrees.
    Angle,
    /// Plain number.
    Plain,
}

/// Parse `s` against the field's current value. Returns None if it doesn't parse.
pub fn eval(s: &str, current: f64, kind: Kind) -> Option<f64> {
    // the field shows its suffix ("20.00 mm"); users often keep it while editing
    let s = s.trim().replace(',', ".");
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    // relative edits
    if let Some(rest) = s.strip_prefix('*') {
        return Some(current * Parser::new(rest, kind).parse_all()?);
    }
    if let Some(rest) = s.strip_prefix('/') {
        let d = Parser::new(rest, kind).parse_all()?;
        return (d != 0.0).then(|| current / d);
    }
    if let Some(rest) = s.strip_suffix('%') {
        if !rest.contains(['+', '-', '*', '/', '(']) {
            return Some(current * Parser::new(rest, Kind::Plain).parse_all()? / 100.0);
        }
    }
    Parser::new(s, kind).parse_all()
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
    kind: Kind,
}

impl<'a> Parser<'a> {
    fn new(s: &'a str, kind: Kind) -> Self {
        Parser { s: s.as_bytes(), i: 0, kind }
    }
    fn ws(&mut self) {
        while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }
    fn peek(&mut self) -> Option<u8> {
        self.ws();
        self.s.get(self.i).copied()
    }
    fn parse_all(mut self) -> Option<f64> {
        let v = self.expr()?;
        self.ws();
        (self.i == self.s.len() && v.is_finite()).then_some(v)
    }
    fn expr(&mut self) -> Option<f64> {
        let mut v = self.term()?;
        loop {
            match self.peek() {
                Some(b'+') => {
                    self.i += 1;
                    v += self.term()?;
                }
                Some(b'-') => {
                    self.i += 1;
                    v -= self.term()?;
                }
                _ => return Some(v),
            }
        }
    }
    fn term(&mut self) -> Option<f64> {
        let mut v = self.factor()?;
        loop {
            match self.peek() {
                Some(b'*') => {
                    self.i += 1;
                    v *= self.factor()?;
                }
                Some(b'/') => {
                    self.i += 1;
                    let d = self.factor()?;
                    if d == 0.0 {
                        return None;
                    }
                    v /= d;
                }
                _ => return Some(v),
            }
        }
    }
    fn factor(&mut self) -> Option<f64> {
        match self.peek()? {
            b'-' => {
                self.i += 1;
                Some(-self.factor()?)
            }
            b'+' => {
                self.i += 1;
                self.factor()
            }
            b'(' => {
                self.i += 1;
                let v = self.expr()?;
                if self.peek()? != b')' {
                    return None;
                }
                self.i += 1;
                Some(v * self.unit())
            }
            _ => {
                let n = self.number()?;
                Some(n * self.unit())
            }
        }
    }
    fn number(&mut self) -> Option<f64> {
        self.ws();
        let start = self.i;
        while self.i < self.s.len() && (self.s[self.i].is_ascii_digit() || self.s[self.i] == b'.') {
            self.i += 1;
        }
        // exponent: 1e3
        if self.i < self.s.len() && (self.s[self.i] == b'e' || self.s[self.i] == b'E') && self.i > start {
            let save = self.i;
            self.i += 1;
            if self.i < self.s.len() && (self.s[self.i] == b'-' || self.s[self.i] == b'+') {
                self.i += 1;
            }
            let ds = self.i;
            while self.i < self.s.len() && self.s[self.i].is_ascii_digit() {
                self.i += 1;
            }
            if self.i == ds {
                self.i = save;
            }
        }
        std::str::from_utf8(&self.s[start..self.i]).ok()?.parse().ok()
    }
    /// Optional unit suffix → factor to the field's base unit.
    fn unit(&mut self) -> f64 {
        self.ws();
        let rest = std::str::from_utf8(&self.s[self.i..]).unwrap_or("");
        let units: &[(&str, f64)] = match self.kind {
            Kind::Length => {
                &[("mm", 1.0), ("cm", 10.0), ("um", 0.001), ("µm", 0.001), ("in", 25.4), ("\"", 25.4), ("ft", 304.8), ("m", 1000.0)]
            }
            Kind::Angle => &[("deg", 1.0), ("°", 1.0), ("rad", 180.0 / std::f64::consts::PI)],
            Kind::Plain => &[],
        };
        for (u, f) in units {
            if rest.starts_with(u) {
                self.i += u.len();
                return *f;
            }
        }
        1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn l(s: &str) -> Option<f64> {
        eval(s, 10.0, Kind::Length)
    }

    #[test]
    fn arithmetic_and_units() {
        assert_eq!(l("20+5"), Some(25.0));
        assert_eq!(l("(10+2)*3"), Some(36.0));
        assert_eq!(l("1in"), Some(25.4));
        assert_eq!(l("2.5 cm"), Some(25.0));
        assert_eq!(l("12,5"), Some(12.5));
        assert_eq!(l("1in + 5mm"), Some(30.4));
        assert_eq!(l("-3"), Some(-3.0));
        assert_eq!(l("20.00 mm"), Some(20.0));
        assert_eq!(l("1e2"), Some(100.0));
        assert_eq!(l("abc"), None);
        assert_eq!(l("5/0"), None);
    }

    #[test]
    fn relative() {
        assert_eq!(l("*2"), Some(20.0));
        assert_eq!(l("/4"), Some(2.5));
        assert_eq!(l("50%"), Some(5.0));
    }

    #[test]
    fn angles() {
        assert_eq!(eval("45°", 0.0, Kind::Angle), Some(45.0));
        assert!((eval("1rad", 0.0, Kind::Angle).unwrap() - 57.29578).abs() < 1e-4);
        assert_eq!(eval("90/2", 0.0, Kind::Angle), Some(45.0));
    }
}
