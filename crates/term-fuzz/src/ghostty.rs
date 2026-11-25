//! libghostty-vt 1.0 as a further core to compare with (E11.3, P0): its screen read cell by cell through the C API
//! into [`Screen`], which the own core's screen is put into too.

use std::fmt::Write as _;

/// A screen as the comparison sees it: text, cursor and the core attributes of each cell.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Screen {
    /// Line and column.
    pub cursor: (usize, usize),
    /// The rows, top first.
    pub rows: Vec<Vec<Cell>>,
}

/// One cell of a [`Screen`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Cell {
    /// The character with its combining ones; empty for the right half of a wide one.
    pub text: String,
    /// Attributes by name, `fg=`/`bg=` with a palette number or `#rrggbb`, `underline=` with SGR 4's kind.
    pub attrs: Vec<String>,
}

impl Screen {
    /// The screen as lines of text, `cursor`, `line` and `cell` lines as in the reference format.
    pub fn render(&self) -> String {
        let mut out = format!("cursor {} {}\n", self.cursor.0, self.cursor.1);
        let mut cells = String::new();
        for (line, row) in self.rows.iter().enumerate() {
            let text: String = row.iter().map(|cell| cell.text.as_str()).collect();
            let _ = writeln!(out, "line {line} {}", text.trim_end());
            for (column, cell) in row.iter().enumerate() {
                if !cell.attrs.is_empty() {
                    let _ = writeln!(cells, "cell {line} {column} {}", cell.attrs.join(" "));
                }
            }
        }
        out + &cells
    }
}

/// libghostty-vt is linked into this build.
pub const AVAILABLE: bool = cfg!(ghostty);

/// The screen libghostty-vt leaves after `bytes` on a terminal of `columns` × `lines`; none without the library.
pub fn screen(bytes: &[u8], columns: u16, lines: u16) -> Option<Screen> {
    #[cfg(ghostty)]
    {
        linked::screen(bytes, columns, lines)
    }
    #[cfg(not(ghostty))]
    {
        let _ = (bytes, columns, lines);
        None
    }
}

#[cfg(ghostty)]
mod linked {
    use std::ffi::c_void;

    use super::{Cell, Screen};

    type Terminal = *mut c_void;

    #[repr(C)]
    #[derive(Default)]
    struct GridRef {
        node: usize,
        x: u16,
        y: u16,
    }

    #[repr(C)]
    struct Coordinate {
        x: u16,
        y: u32,
    }

    #[repr(C)]
    union PointValue {
        coordinate: std::mem::ManuallyDrop<Coordinate>,
        padding: [u64; 2],
    }

    #[repr(C)]
    struct Point {
        tag: i32,
        value: PointValue,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct StyleColor {
        tag: u8,
        value: [u8; 3],
    }

    #[repr(C)]
    #[derive(Default)]
    struct Style {
        fg: StyleColor,
        bg: StyleColor,
        underline: StyleColor,
        flags: u16,
        reserved: u16,
    }

    const SUCCESS: i32 = 0;
    const POINT_ACTIVE: i32 = 0;
    const DATA_ROWS: i32 = 2;
    const DATA_CURSOR_X: i32 = 3;
    const DATA_CURSOR_Y: i32 = 4;
    const CELL_CONTENT_TAG: i32 = 2;
    const CELL_WIDE: i32 = 3;
    const CELL_COLOR_PALETTE: i32 = 10;
    const CELL_COLOR_RGB: i32 = 11;
    const CONTENT_BG_PALETTE: i32 = 2;
    const CONTENT_BG_RGB: i32 = 3;
    const WIDE_WIDE: i32 = 1;
    const WIDE_SPACER_TAIL: i32 = 2;
    const COLOR_PALETTE: u8 = 1;
    const COLOR_RGB: u8 = 2;
    const FLAGS: [(u16, &str); 6] = [
        (1, "bold"),
        (1 << 1, "italic"),
        (1 << 2, "dim"),
        (1 << 4, "inverse"),
        (1 << 5, "hidden"),
        (1 << 6, "strike"),
    ];

    // SAFETY: These are the declarations of `ghostty/vt.h` of libghostty-vt 1.0.1, which `build.rs` links.
    unsafe extern "C" {
        fn ghostty_terminal_new(allocator: *const c_void, terminal: *mut Terminal, cols: u16, rows: u16) -> i32;
        fn ghostty_terminal_free(terminal: Terminal);
        fn ghostty_terminal_vt_write(terminal: Terminal, data: *const u8, len: usize);
        fn ghostty_terminal_get(terminal: Terminal, data: i32, out: *mut c_void) -> i32;
        fn ghostty_terminal_grid_ref(terminal: Terminal, point: Point, out: *mut GridRef) -> i32;
        fn ghostty_grid_ref_cell(grid_ref: *const GridRef, out: *mut u64) -> i32;
        fn ghostty_grid_ref_style(grid_ref: *const GridRef, out: *mut Style) -> i32;
        fn ghostty_grid_ref_graphemes(grid_ref: *const GridRef, buf: *mut u32, len: usize, written: *mut usize) -> i32;
        fn ghostty_cell_get(cell: u64, data: i32, out: *mut c_void) -> i32;
    }

    struct Owned(Terminal);

    impl Drop for Owned {
        fn drop(&mut self) {
            // SAFETY: The terminal came from `ghostty_terminal_new` and is freed once.
            unsafe { ghostty_terminal_free(self.0) };
        }
    }

    pub(super) fn screen(bytes: &[u8], columns: u16, lines: u16) -> Option<Screen> {
        let mut terminal: Terminal = std::ptr::null_mut();
        // SAFETY: A null allocator asks for the default one; the out pointer is valid.
        if unsafe { ghostty_terminal_new(std::ptr::null(), &raw mut terminal, columns, lines) } != SUCCESS {
            return None;
        }
        let terminal = Owned(terminal);
        // SAFETY: The terminal is alive and the slice valid for its length.
        unsafe { ghostty_terminal_vt_write(terminal.0, bytes.as_ptr(), bytes.len()) };
        let get = |data: i32| -> u16 {
            let mut value: u16 = 0;
            // SAFETY: Rows and cursor position are written as `uint16_t`.
            unsafe { ghostty_terminal_get(terminal.0, data, (&raw mut value).cast()) };
            value
        };
        let mut screen = Screen {
            cursor: (usize::from(get(DATA_CURSOR_Y)), usize::from(get(DATA_CURSOR_X))),
            rows: Vec::new(),
        };
        for y in 0..get(DATA_ROWS) {
            let row = (0..columns)
                .map(|x| cell(terminal.0, x, y))
                .collect::<Option<Vec<_>>>()?;
            screen.rows.push(row);
        }
        Some(screen)
    }

    fn cell(terminal: Terminal, x: u16, y: u16) -> Option<Cell> {
        let point = Point {
            tag: POINT_ACTIVE,
            value: PointValue {
                coordinate: std::mem::ManuallyDrop::new(Coordinate { x, y: u32::from(y) }),
            },
        };
        let mut grid_ref = GridRef::default();
        // SAFETY: The terminal is alive and the out pointer valid.
        if unsafe { ghostty_terminal_grid_ref(terminal, point, &raw mut grid_ref) } != SUCCESS {
            return None;
        }
        let mut raw: u64 = 0;
        let mut style = Style::default();
        // SAFETY: The reference was just made and the terminal not written since; the out pointers are valid.
        unsafe {
            ghostty_grid_ref_cell(&raw const grid_ref, &raw mut raw);
            ghostty_grid_ref_style(&raw const grid_ref, &raw mut style);
        }
        let get = |data: i32, out: *mut c_void| {
            // SAFETY: Each caller passes the type the header names for `data`.
            unsafe { ghostty_cell_get(raw, data, out) }
        };
        let (mut content, mut wide) = (0i32, 0i32);
        get(CELL_CONTENT_TAG, (&raw mut content).cast());
        get(CELL_WIDE, (&raw mut wide).cast());
        let mut codepoints = [0u32; 16];
        let mut written = 0usize;
        // SAFETY: The buffer holds 16 codepoints; a longer cluster returns an error and writes nothing.
        unsafe {
            ghostty_grid_ref_graphemes(
                &raw const grid_ref,
                codepoints.as_mut_ptr(),
                codepoints.len(),
                &raw mut written,
            );
        }
        let text: String = codepoints[..written.min(16)]
            .iter()
            .filter_map(|&c| char::from_u32(c))
            .collect();
        let mut attrs = Vec::new();
        if let Some(fg) = color(style.fg) {
            attrs.push(format!("fg={fg}"));
        }
        let bg = match content {
            CONTENT_BG_PALETTE => {
                let mut index = 0u8;
                get(CELL_COLOR_PALETTE, (&raw mut index).cast());
                Some(index.to_string())
            }
            CONTENT_BG_RGB => {
                let mut rgb = [0u8; 3];
                get(CELL_COLOR_RGB, rgb.as_mut_ptr().cast());
                Some(format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2]))
            }
            _ => color(style.bg),
        };
        if let Some(bg) = bg {
            attrs.push(format!("bg={bg}"));
        }
        attrs.extend(
            FLAGS
                .iter()
                .filter(|(bit, _)| style.flags & bit != 0)
                .map(|(_, name)| (*name).to_string()),
        );
        let underline = (style.flags >> 8) & 0x7;
        if underline != 0 {
            attrs.push(format!("underline={underline}"));
        }
        if wide == WIDE_WIDE {
            attrs.push("wide".into());
        }

        let text = match (wide, text.is_empty()) {
            (WIDE_SPACER_TAIL, _) => String::new(),
            (_, true) => " ".into(),
            _ => text,
        };
        Some(Cell { text, attrs })
    }

    fn color(color: StyleColor) -> Option<String> {
        match color.tag {
            COLOR_PALETTE => Some(color.value[0].to_string()),
            COLOR_RGB => Some(format!(
                "#{:02x}{:02x}{:02x}",
                color.value[0], color.value[1], color.value[2]
            )),
            _ => None,
        }
    }
}
