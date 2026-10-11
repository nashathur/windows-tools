//! Small single-line edit model used by Block panels.

#[derive(Clone, Debug, Default, PartialEq, Eq)]
/// The text, caret position and select-all state for a panel search field.
pub struct TextEdit {
    text: String,
    caret: usize,
    selected_all: bool,
}

impl TextEdit {
    /// Creates an edit model with its caret at the end of the initial text.
    pub fn new(text: impl Into<String>) -> Self {
        let text = text.into();
        let caret = text.chars().count();
        Self {
            text,
            caret,
            selected_all: false,
        }
    }
    /// Returns the current search text.
    pub fn text(&self) -> &str {
        &self.text
    }
    /// Returns the caret position measured in Unicode scalar values.
    pub fn caret(&self) -> usize {
        self.caret
    }
    /// Reports whether the whole value is selected.
    pub fn all_selected(&self) -> bool {
        self.selected_all
    }
    /// Selects the whole value for replacement or copying.
    pub fn select_all(&mut self) {
        self.selected_all = true;
        self.caret = self.text.chars().count();
    }
    /// Inserts text at the caret, replacing the whole value when selected.
    pub fn insert(&mut self, text: &str) {
        self.replace_selection();
        let at = byte_at(&self.text, self.caret);
        self.text.insert_str(at, text);
        self.caret += text.chars().count();
    }
    /// Deletes the character before the caret or the selected value.
    pub fn backspace(&mut self) {
        if self.replace_selection() {
            return;
        }
        if self.caret > 0 {
            let end = byte_at(&self.text, self.caret);
            let start = byte_at(&self.text, self.caret - 1);
            self.text.replace_range(start..end, "");
            self.caret -= 1;
        }
    }
    /// Deletes the character at the caret or the selected value.
    pub fn delete(&mut self) {
        if self.replace_selection() {
            return;
        }
        let n = self.text.chars().count();
        if self.caret < n {
            let start = byte_at(&self.text, self.caret);
            let end = byte_at(&self.text, self.caret + 1);
            self.text.replace_range(start..end, "");
        }
    }
    /// Moves the caret one character left and collapses selection to the start.
    pub fn left(&mut self) {
        if self.selected_all {
            self.selected_all = false;
            self.caret = 0;
        } else {
            self.caret = self.caret.saturating_sub(1);
        }
    }
    /// Moves the caret one character right and collapses selection to the end.
    pub fn right(&mut self) {
        if self.selected_all {
            self.selected_all = false;
            self.caret = self.text.chars().count();
        } else {
            self.caret = (self.caret + 1).min(self.text.chars().count());
        }
    }
    /// Moves the caret to the beginning of the value.
    pub fn home(&mut self) {
        self.selected_all = false;
        self.caret = 0;
    }
    /// Moves the caret to the end of the value.
    pub fn end(&mut self) {
        self.selected_all = false;
        self.caret = self.text.chars().count();
    }
    /// Deletes the previous word and its preceding whitespace.
    pub fn ctrl_backspace(&mut self) {
        if self.replace_selection() {
            return;
        }
        let chars: Vec<char> = self.text.chars().collect();
        let mut start = self.caret;
        while start > 0 && chars[start - 1].is_whitespace() {
            start -= 1;
        }
        while start > 0 && !chars[start - 1].is_whitespace() {
            start -= 1;
        }
        let a = byte_at(&self.text, start);
        let b = byte_at(&self.text, self.caret);
        self.text.replace_range(a..b, "");
        self.caret = start;
    }
    /// Returns the selected text for copying.
    pub fn copy(&self) -> String {
        if self.selected_all {
            self.text.clone()
        } else {
            String::new()
        }
    }
    /// Removes the selected text and returns it for copying.
    pub fn cut(&mut self) -> String {
        let text = self.copy();
        self.replace_selection();
        text
    }
    /// Inserts pasted text after removing line breaks and null characters.
    pub fn paste(&mut self, text: &str) {
        self.insert(&text.replace(['\r', '\n', '\0'], ""));
    }
    /// Removes a select-all range and reports whether a selection was replaced.
    fn replace_selection(&mut self) -> bool {
        if !self.selected_all {
            return false;
        }
        self.text.clear();
        self.caret = 0;
        self.selected_all = false;
        true
    }
}

/// Converts a character offset into a byte offset in a UTF-8 string.
fn byte_at(s: &str, chars: usize) -> usize {
    s.char_indices().nth(chars).map_or(s.len(), |(i, _)| i)
}

/// Copies Unicode text using the Win32 clipboard. `owner` must be a window of this process:
/// with none, EmptyClipboard leaves the clipboard ownerless and SetClipboardData fails.
pub fn clipboard_write(owner: isize, text: &str) {
    use windows_sys::Win32::System::{DataExchange::*, Memory::*};
    const CF_UNICODETEXT: u32 = 13;
    unsafe extern "system" {
        fn GlobalFree(mem: *mut std::ffi::c_void) -> *mut std::ffi::c_void;
    }
    unsafe {
        if OpenClipboard(owner) == 0 {
            return;
        }
        EmptyClipboard();
        let mut wide: Vec<u16> = text.encode_utf16().collect();
        wide.push(0);
        let bytes = wide.len() * std::mem::size_of::<u16>();
        let mem = GlobalAlloc(GMEM_MOVEABLE, bytes);
        if !mem.is_null() {
            let ptr = GlobalLock(mem) as *mut u16;
            if !ptr.is_null() {
                std::ptr::copy_nonoverlapping(wide.as_ptr(), ptr, wide.len());
                GlobalUnlock(mem);
                if SetClipboardData(CF_UNICODETEXT, mem as _) == 0 {
                    GlobalFree(mem);
                }
            } else {
                GlobalFree(mem);
            }
        }
        CloseClipboard();
    }
}

/// Reads Unicode text from the Win32 clipboard.
pub fn clipboard_read() -> Option<String> {
    use windows_sys::Win32::System::{DataExchange::*, Memory::*};
    const CF_UNICODETEXT: u32 = 13;
    unsafe {
        if OpenClipboard(0) == 0 {
            return None;
        }
        let handle = GetClipboardData(CF_UNICODETEXT as u32);
        let mut result = None;
        if handle != 0 {
            let ptr = GlobalLock(handle as _) as *const u16;
            if !ptr.is_null() {
                let mut len = 0;
                while *ptr.add(len) != 0 {
                    len += 1;
                }
                result = Some(String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len)));
                GlobalUnlock(handle as _);
            }
        }
        CloseClipboard();
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    /// Checks Unicode editing, select-all, cut and paste behavior.
    #[test]
    fn edits_unicode_and_selection() {
        let mut e = TextEdit::new("Büs stop");
        e.home();
        e.insert("Utrecht ");
        assert_eq!(e.text(), "Utrecht Büs stop");
        e.select_all();
        assert_eq!(e.cut(), "Utrecht Büs stop");
        e.paste("Rhône\nplein");
        assert_eq!(e.text(), "Rhôneplein");
    }
    /// Checks caret editing and deletion of the previous word.
    #[test]
    fn edits_at_caret_and_ctrl_backspace() {
        let mut e = TextEdit::new("one two");
        e.ctrl_backspace();
        assert_eq!(e.text(), "one ");
        e.backspace();
        assert_eq!(e.text(), "one");
        e.home();
        e.delete();
        assert_eq!(e.text(), "ne");
    }
    /// Checks that arrow keys collapse select-all to the appropriate edge.
    #[test]
    fn selection_collapses_to_the_expected_edge_on_arrow_keys() {
        let mut e = TextEdit::new("hello");
        e.select_all();
        e.left();
        assert_eq!(e.caret(), 0);
        e.select_all();
        e.right();
        assert_eq!(e.caret(), 5);
    }
}
