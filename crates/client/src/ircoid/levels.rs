//! Ircoid's six levels, as text maps: one character per brick, thirteen to
//! a row, at most ten rows. After the sixth they repeat, still faster.
//!
//! - `m` a message, `@` an op, `~` an owner, `#` a ban, `.` nothing.

use super::{Brick, BrickKind};

/// Bricks in a row.
pub const COLUMNS: usize = 13;
/// Rows in a level, at most.
pub const ROWS: usize = 10;

/// A level: its channel and its map.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layout {
    pub channel: &'static str,
    pub map: &'static [&'static str],
}

/// The six layouts, in order.
pub const LAYOUTS: [Layout; 6] = [
    // The IRCord mark: two uprights, two bars, ops where they cross.
    Layout {
        channel: "#the-hash",
        map: &[
            "....mm...mm..",
            "....mm...mm..",
            "mmmm@@mmm@@mm",
            "....mm...mm..",
            "....mm...mm..",
            "mmmm@@mmm@@mm",
            "....mm...mm..",
        ],
    },
    // A framed box of ops around rows of messages, like a server's MOTD.
    Layout {
        channel: "#motd",
        map: &[
            "@@@@@@@@@@@@@",
            "@mmmmmmmmmmm@",
            "@m~mmmmmmm~m@",
            "@mmmmmmmmmmm@",
            "@mmmmmmmmmmm@",
            "@@@@@@@@@@@@@",
        ],
    },
    // Two halves split by a column of bans with two gaps.
    Layout {
        channel: "#netsplit",
        map: &[
            "mmmmmm#mmmmmm",
            "mmmmmm#mmmmmm",
            "mmmmmm.mmmmmm",
            "mm@mmm#mmm@mm",
            "mmmmmm#mmmmmm",
            "mmmmmm.mmmmmm",
            "mmmmmm#mmmmmm",
        ],
    },
    // A large @.
    Layout {
        channel: "#ops",
        map: &[
            "..@@@@@@@@@..",
            ".@mmmmmmmmm@.",
            "@mm@@@@@mmm@.",
            "@m@mmmmm@mm@.",
            "@m@m~~~m@mm@.",
            "@m@mmmmm@mm@.",
            "@mm@@@@@@@@..",
            ".@mmmmmmmmmm.",
            "..@@@@@@@@@@.",
        ],
    },
    // Rows of messages behind staggered ban bars.
    Layout {
        channel: "#banlist",
        map: &[
            "m~mmmmmmmmm~m",
            "mmmmmmmmmmmmm",
            "####.....####",
            "mmmmmmmmmmmmm",
            "mmmmmmmmmmmmm",
            "....#####....",
            "mmmmmmmmmmmmm",
            "@mmmmmmmmmmm@",
        ],
    },
    // Diagonals, with owners at the ends.
    Layout {
        channel: "#k-line",
        map: &[
            "~...........~",
            "m@.........@m",
            ".mm.......mm.",
            "..mm.....mm..",
            "...mm...mm...",
            "....mm.mm....",
            ".....m@m.....",
            "......~......",
        ],
    },
];

/// The layout for `level`, from 1; they repeat.
pub fn layout(level: u32) -> &'static Layout {
    &LAYOUTS[(level.max(1) as usize - 1) % LAYOUTS.len()]
}

/// The bricks of a map.
pub fn parse(map: &[&str]) -> Vec<Brick> {
    let mut bricks = Vec::new();
    for (row, line) in map.iter().enumerate() {
        for (col, c) in line.chars().enumerate() {
            let kind = match c {
                'm' => BrickKind::Message,
                '@' => BrickKind::Op,
                '~' => BrickKind::Owner,
                '#' => BrickKind::Ban,
                _ => continue,
            };
            bricks.push(Brick::new(col as u8, row as u8, kind));
        }
    }
    bricks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_map_fits_and_can_be_cleared() {
        for layout in &LAYOUTS {
            assert!(layout.map.len() <= ROWS, "{}", layout.channel);
            for line in layout.map {
                assert_eq!(line.chars().count(), COLUMNS, "{}: {line}", layout.channel);
                assert!(line.chars().all(|c| "m@~#.".contains(c)));
            }
            let bricks = parse(layout.map);
            assert!(
                bricks.iter().any(|b| b.kind != BrickKind::Ban),
                "{}",
                layout.channel
            );
        }
    }

    #[test]
    fn level_one_is_the_mark() {
        let bricks = parse(layout(1).map);
        let count = |kind| bricks.iter().filter(|b| b.kind == kind).count();
        assert_eq!(count(BrickKind::Message), 38);
        assert_eq!(count(BrickKind::Op), 8);
        assert_eq!(count(BrickKind::Owner), 0);
        assert_eq!(count(BrickKind::Ban), 0);
    }

    #[test]
    fn the_layouts_repeat() {
        assert_eq!(layout(7), layout(1));
        assert_eq!(layout(12).channel, "#k-line");
        assert_eq!(layout(0), layout(1));
    }
}
