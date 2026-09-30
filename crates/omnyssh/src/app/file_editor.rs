//! In-app remote file editor state (ViewState only).
//!
//! Small files are edited here; large files use [`PendingExternalEdit`] + `$EDITOR`.

// ---------------------------------------------------------------------------
// UTF-8 helpers (byte index ↔ char boundary)
// ---------------------------------------------------------------------------

/// Previous char boundary before or at `idx`.
fn prev_char_boundary(s: &str, idx: usize) -> usize {
    if idx == 0 {
        return 0;
    }

    let mut i = idx.min(s.len()).saturating_sub(1);

    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }

    i
}

/// Clamp a byte index to a valid UTF-8 character boundary.
fn clamp_col(s: &str, col: usize) -> usize {
    let col = col.min(s.len());

    if s.is_char_boundary(col) {
        col
    } else {
        prev_char_boundary(s, col)
    }
}

/// Open editor session for a remote file under the in-app size cap.
#[derive(Debug, Clone)]
pub struct FileEditorView {
    pub remote_path: String,

    /// Buffer as individual lines (v1 simple model).
    pub lines: Vec<String>,

    pub cursor_row: usize,
    pub cursor_col: usize,

    pub dirty: bool,
    pub saving: bool,
}

impl FileEditorView {
    pub fn from_content(path: String, content: String) -> Self {
        let lines: Vec<String> = if content.is_empty() {
            vec![String::new()]
        } else {
            content.lines().map(str::to_owned).collect()
        };

        Self {
            remote_path: path,
            lines,
            cursor_row: 0,
            cursor_col: 0,
            dirty: false,
            saving: false,
        }
    }

    /// Serialize buffer back to a single string (always ends with `\n` if non-empty).
    pub fn to_content(&self) -> String {
        let mut s = self.lines.join("\n");

        if !self.lines.is_empty() {
            s.push('\n');
        }

        s
    }

    /// Keep cursor row and column within the current buffer.
    ///
    /// Cursor positions are byte indexes, but they must always point
    /// to valid UTF-8 character boundaries.
    pub fn clamp_cursor(&mut self) {
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }

        self.cursor_row = self.cursor_row.min(self.lines.len() - 1);

        let line = &self.lines[self.cursor_row];
        self.cursor_col = clamp_col(line, self.cursor_col);
    }
}

/// Pending external-edit session: download → user editor → upload if changed.
#[derive(Debug, Clone)]
pub struct PendingExternalEdit {
    pub remote_path: String,
    pub local_path: String,

    /// Transfer identifier used by the external transfer workflow.
    ///
    /// Currently stored for the transfer workflow but not read directly
    /// by this module.
    #[allow(dead_code)]
    pub transfer_id: u64,

    /// Bytes right after download. `None` means download still in progress.
    pub pre_bytes: Option<Vec<u8>>,
}
