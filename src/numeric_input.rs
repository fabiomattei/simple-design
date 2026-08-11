/// Which edge/center of a layer's frame a resize/position value should be
/// measured from, per the Inspector's keyboard operators (`…l`/`…r`/
/// `…t`/`…b`/`…c`/`…m` — see `ui/inspector.rs`'s Position/Size fields).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Anchor {
    Left,
    Right,
    Top,
    Bottom,
    Center,
}

/// Parses one Inspector dimension field's raw typed text into a numeric
/// value plus an optional anchor override, per the following syntax:
/// - A leading `+`, `-`, `*`, or `/` makes the number relative to `current`
///   (e.g. typing `-10` sets the value to `current - 10`, not literal `-10`
///   — matches a common "Mathematical Operations" behavior).
/// - A trailing `%` makes the number a percentage of `extent_for_pct` (the
///   nearest parent container's width/height — see CLAUDE.md's "parent-
///   relative" percentage convention).
/// - A trailing anchor letter (`l`/`r`/`t`/`b`/`c`/`m`) is stripped and
///   returned separately — callers apply it to decide which edge stays
///   fixed (Size fields) or which edge the value positions (Position
///   fields).
///
/// Not implemented: the `w`/`h`/`x`/`y` field-substitution letters (typing
/// e.g. `50w` into the Y field to set width instead) — a rarely-used power-
/// user shortcut that would require routing edits across fields; every
/// other documented operator above is supported.
///
/// Returns `None` if the text doesn't parse as a number at all (empty
/// field, garbage input, division by zero) — callers should leave the
/// field's value unchanged in that case, not clear it.
pub fn parse_dimension_expr(current: f64, raw: &str, extent_for_pct: f64) -> Option<(f64, Option<Anchor>)> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let (num_part, anchor) = split_trailing_anchor(raw);
    let num_part = num_part.trim();
    if num_part.is_empty() {
        return None;
    }

    let value = if let Some(rest) = num_part.strip_prefix('+') {
        current + parse_plain(rest)?
    } else if let Some(rest) = num_part.strip_prefix('-') {
        current - parse_plain(rest)?
    } else if let Some(rest) = num_part.strip_prefix('*') {
        current * parse_plain(rest)?
    } else if let Some(rest) = num_part.strip_prefix('/') {
        let divisor = parse_plain(rest)?;
        if divisor == 0.0 {
            return None;
        }
        current / divisor
    } else if let Some(pct) = num_part.strip_suffix('%') {
        extent_for_pct * parse_plain(pct)? / 100.0
    } else {
        parse_plain(num_part)?
    };
    Some((value, anchor))
}

fn parse_plain(s: &str) -> Option<f64> {
    s.trim().parse::<f64>().ok()
}

/// Splits a single trailing anchor letter off `s`, if its last character is
/// one of `l`/`r`/`t`/`b`/`c`/`m` (case-insensitive). Anything else (no
/// trailing letter, or a letter that isn't a recognized anchor — e.g. an
/// `e` from scientific notation like `1e5`) is left untouched.
fn split_trailing_anchor(s: &str) -> (&str, Option<Anchor>) {
    let Some(last) = s.chars().last() else { return (s, None) };
    if !last.is_ascii_alphabetic() {
        return (s, None);
    }
    let anchor = match last.to_ascii_lowercase() {
        'l' => Anchor::Left,
        'r' => Anchor::Right,
        't' => Anchor::Top,
        'b' => Anchor::Bottom,
        'c' | 'm' => Anchor::Center,
        _ => return (s, None),
    };
    (&s[..s.len() - last.len_utf8()], Some(anchor))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_number_ignores_current_and_extent() {
        assert_eq!(parse_dimension_expr(100.0, "50", 1000.0), Some((50.0, None)));
    }

    #[test]
    fn leading_operators_are_relative_to_current() {
        assert_eq!(parse_dimension_expr(100.0, "+10", 0.0), Some((110.0, None)));
        assert_eq!(parse_dimension_expr(100.0, "-10", 0.0), Some((90.0, None)));
        assert_eq!(parse_dimension_expr(100.0, "*2", 0.0), Some((200.0, None)));
        assert_eq!(parse_dimension_expr(100.0, "/4", 0.0), Some((25.0, None)));
    }

    #[test]
    fn division_by_zero_is_none() {
        assert_eq!(parse_dimension_expr(100.0, "/0", 0.0), None);
    }

    #[test]
    fn percent_is_relative_to_extent() {
        assert_eq!(parse_dimension_expr(0.0, "50%", 200.0), Some((100.0, None)));
    }

    #[test]
    fn trailing_anchor_letter_is_split_off() {
        assert_eq!(parse_dimension_expr(0.0, "50r", 0.0), Some((50.0, Some(Anchor::Right))));
        assert_eq!(parse_dimension_expr(0.0, "50c", 0.0), Some((50.0, Some(Anchor::Center))));
        assert_eq!(parse_dimension_expr(0.0, "50m", 0.0), Some((50.0, Some(Anchor::Center))));
        assert_eq!(parse_dimension_expr(0.0, "50t", 0.0), Some((50.0, Some(Anchor::Top))));
        assert_eq!(parse_dimension_expr(0.0, "50b", 0.0), Some((50.0, Some(Anchor::Bottom))));
    }

    #[test]
    fn anchor_letter_combines_with_operators_and_percent() {
        assert_eq!(parse_dimension_expr(100.0, "+10r", 0.0), Some((110.0, Some(Anchor::Right))));
        assert_eq!(parse_dimension_expr(0.0, "50%r", 200.0), Some((100.0, Some(Anchor::Right))));
    }

    #[test]
    fn empty_or_garbage_input_is_none() {
        assert_eq!(parse_dimension_expr(100.0, "", 0.0), None);
        assert_eq!(parse_dimension_expr(100.0, "abc", 0.0), None);
        assert_eq!(parse_dimension_expr(100.0, "l", 0.0), None);
    }
}
