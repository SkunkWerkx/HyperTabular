//! The block walker's output as arrays — Sep's packed layout: `cells` is one array of
//! cell ends across all rows, and each [`RowInfo`] names its first cell, its cell count,
//! and its start offset, so cell `j` of a row spans `[prev_end + 1, end_j)` with
//! `prev_end + 1 = row.start` for `j = 0`.
//!
//! The walker itself is the allocation-free core's
//! ([`crate::kernel::delimited::scan`]); this is the sink that collects what it reports
//! into vectors, for the row-at-a-time [`crate::delimited::Reader`].

pub use crate::kernel::delimited::scan::{Flow, Scanner, Sink, Stop};

/// One cell's end: the offset of the byte that terminated it (a separator, `\r`, `\n`,
/// or the data end), and the number of `"` bytes it contained.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CellEnd {
    pub end: u32,
    pub quotes: u32,
}

/// One complete row.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RowInfo {
    /// Offset of the row's first byte.
    pub start: u32,
    /// Index of the row's first cell in [`Output::cells`].
    pub first_cell: u32,
    /// Number of cells.
    pub cells: u32,
    /// One-based line the row starts on.
    pub line: u32,
    /// Length of the row terminator: 0 at the data end, 1 for `\n` or `\r`, 2 for `\r\n`.
    pub terminator: u8,
}

/// The walker's output for one scan — reused across scans.
#[derive(Clone, Debug, Default)]
pub struct Output {
    pub cells: Vec<CellEnd>,
    pub rows: Vec<RowInfo>,
}

impl Output {
    /// Empties both arrays, keeping their capacity.
    pub fn clear(&mut self) {
        self.cells.clear();
        self.rows.clear();
    }
}

/// Appends rows to an [`Output`] until `remaining` have been taken.
struct Collect<'a> {
    out: &'a mut Output,
    first_cell: usize,
    remaining: usize,
}

impl Sink for Collect<'_> {
    fn cell(&mut self, _start: usize, end: usize, quotes: u32) {
        self.out.cells.push(CellEnd {
            end: end as u32,
            quotes,
        });
    }

    fn discard(&mut self) {
        self.out.cells.truncate(self.first_cell);
    }

    fn row(&mut self, start: usize, _next: usize, line: u32, terminator: u8) -> Flow {
        self.out.rows.push(RowInfo {
            start: start as u32,
            first_cell: self.first_cell as u32,
            cells: (self.out.cells.len() - self.first_cell) as u32,
            line,
            terminator,
        });
        self.first_cell = self.out.cells.len();
        self.remaining -= 1;
        if self.remaining == 0 {
            Flow::Full
        } else {
            Flow::More
        }
    }
}

impl Scanner {
    /// Scans `data[pos..]`, which must start at a row boundary, appending up to
    /// `max_rows` complete rows to `out`. `line` is the one-based line number at `pos`;
    /// `eof` says whether `data` is all there is (so a final row without a terminator can
    /// be closed at the data end).
    pub fn scan(
        &self,
        data: &[u8],
        pos: usize,
        line: u32,
        eof: bool,
        max_rows: usize,
        out: &mut Output,
    ) -> Stop {
        if max_rows == 0 {
            return Stop {
                next: pos,
                partial: false,
                line,
                unclosed: false,
            };
        }
        let mut sink = Collect {
            first_cell: out.cells.len(),
            out,
            remaining: max_rows,
        };
        self.scan_into(data, pos, line, eof, &mut sink)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::delimited::engine;

    /// `(start, end, quotes)` per cell, per row.
    type Cells = Vec<Vec<(usize, usize, u32)>>;

    fn rows(data: &[u8], quoting: bool, eof: bool) -> (Cells, Stop) {
        let mut all = Vec::new();
        for kind in engine::available() {
            let scanner = Scanner::with_engine(b',', quoting, kind);
            let mut out = Output::default();
            let s = scanner.scan(data, 0, 1, eof, usize::MAX, &mut out);
            let mut got = Vec::new();
            for row in &out.rows {
                let mut cells = Vec::new();
                let mut start = row.start as usize;
                for j in 0..row.cells as usize {
                    let cell = out.cells[row.first_cell as usize + j];
                    cells.push((start, cell.end as usize, cell.quotes));
                    start = cell.end as usize + 1;
                }
                got.push(cells);
            }
            all.push((kind, got, s));
        }
        let (_, first_rows, first_stop) = &all[0];
        for (kind, got, s) in &all[1..] {
            assert_eq!(got, first_rows, "engine {kind:?} disagrees on rows");
            assert_eq!(s, first_stop, "engine {kind:?} disagrees on stop");
        }
        (first_rows.clone(), *first_stop)
    }

    #[test]
    fn plain_rows_with_every_terminator() {
        let (r, stop) = rows(b"a,b\nc,d\r\ne,f\rg,h", true, true);
        assert_eq!(
            r,
            vec![
                vec![(0, 1, 0), (2, 3, 0)],
                vec![(4, 5, 0), (6, 7, 0)],
                vec![(9, 10, 0), (11, 12, 0)],
                vec![(13, 14, 0), (15, 16, 0)],
            ]
        );
        assert_eq!(
            stop,
            Stop {
                next: 16,
                partial: false,
                line: 5,
                unclosed: false
            }
        );
    }

    #[test]
    fn quotes_hide_structure_and_are_counted_per_cell() {
        let (r, stop) = rows(b"\"a,b\",\"c\"\"d\"\n\"x\ny\",z\n", true, true);
        assert_eq!(
            r,
            vec![vec![(0, 5, 2), (6, 12, 4)], vec![(13, 18, 2), (19, 20, 0)]]
        );
        assert_eq!(stop.line, 4);
        assert!(!stop.unclosed);
        let (r, _) = rows(b"\"a,b\",c\n", false, true);
        assert_eq!(r, vec![vec![(0, 2, 0), (3, 5, 0), (6, 7, 0)]]);
    }

    #[test]
    fn unclosed_quote_at_eof_is_reported_not_delivered() {
        let (r, stop) = rows(b"a,b\n\"open,c\n", true, true);
        assert_eq!(r.len(), 1);
        assert!(stop.unclosed);
        assert_eq!(stop.next, 4);
    }

    #[test]
    fn crlf_across_a_block_boundary_is_one_terminator() {
        let mut data = vec![b'x'; 63];
        data.push(b'\r');
        data.push(b'\n');
        data.extend_from_slice(b"y\n");
        let (r, _) = rows(&data, true, true);
        assert_eq!(r, vec![vec![(0, 63, 0)], vec![(65, 66, 0)]]);
        // Separator at bit 63 and quote spanning blocks.
        let mut data = vec![b'x'; 63];
        data.push(b',');
        data.extend_from_slice(b"\"q\"\n");
        let (r, _) = rows(&data, true, true);
        assert_eq!(r, vec![vec![(0, 63, 0), (64, 67, 2)]]);
        let mut data = vec![b'"'; 1];
        data.extend(std::iter::repeat_n(b'x', 70));
        data.extend_from_slice(b"\",a\n");
        let (r, _) = rows(&data, true, true);
        assert_eq!(r, vec![vec![(0, 72, 2), (73, 74, 0)]]);
    }

    #[test]
    fn a_trailing_cr_waits_for_more_data_unless_eof() {
        let mut data = vec![b'x'; 63];
        data.push(b'\r');
        let (r, stop) = rows(&data, true, false);
        assert!(r.is_empty());
        assert_eq!(
            stop,
            Stop {
                next: 0,
                partial: true,
                line: 1,
                unclosed: false
            }
        );
        let (r, stop) = rows(&data, true, true);
        assert_eq!(r, vec![vec![(0, 63, 0)]]);
        assert_eq!(stop.next, 64);
        // A short trailing CR (not at bit 63) behaves the same way.
        let (r, stop) = rows(b"a,b\r", true, false);
        assert!(r.is_empty());
        assert!(stop.partial);
    }

    #[test]
    fn partial_rows_are_withheld_when_more_data_may_come() {
        let (r, stop) = rows(b"a,b\nc,d", true, false);
        assert_eq!(r.len(), 1);
        assert_eq!(
            stop,
            Stop {
                next: 4,
                partial: true,
                line: 2,
                unclosed: false
            }
        );
        let (r, stop) = rows(b"a,b\n", true, false);
        assert_eq!(r.len(), 1);
        assert_eq!(
            stop,
            Stop {
                next: 4,
                partial: false,
                line: 2,
                unclosed: false
            }
        );
    }

    #[test]
    fn max_rows_stops_at_a_boundary_and_resumes_cleanly() {
        let data = b"a,b\r\nc,d\ne,f\n";
        let scanner = Scanner::new(b',', true);
        let mut out = Output::default();
        let stop = scanner.scan(data, 0, 1, true, 1, &mut out);
        assert_eq!(out.rows.len(), 1);
        assert_eq!(
            stop,
            Stop {
                next: 5,
                partial: false,
                line: 2,
                unclosed: false
            }
        );
        out.clear();
        let stop = scanner.scan(data, stop.next, stop.line, true, 10, &mut out);
        assert_eq!(out.rows.len(), 2);
        assert_eq!(out.rows[0].start, 5);
        assert_eq!(out.rows[0].line, 2);
        assert_eq!(stop.next, data.len());
    }

    #[test]
    fn empty_lines_are_single_empty_cells_and_the_data_end_closes_a_row() {
        let (r, _) = rows(b"\n\na", true, true);
        assert_eq!(r, vec![vec![(0, 0, 0)], vec![(1, 1, 0)], vec![(2, 3, 0)]]);
        let (r, stop) = rows(b"", true, true);
        assert!(r.is_empty());
        assert_eq!(stop.next, 0);
    }
}
