use crate::{clipboard::Clipboard, themes::Theme};

pub struct Context {
    previous_searches: Vec<Search>,
    clipboard: Clipboard,
    clipboard_content: Option<String>,
    pub theme: Theme,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Search {
    pub kind: SearchKind,
    pub search: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Copy)]
pub enum SearchKind {
    Literal,
    Regex,
    AstGrep,
}

impl Default for Context {
    fn default() -> Self {
        Self {
            previous_searches: Vec::new(),
            clipboard: Clipboard::new(),
            clipboard_content: None,
            theme: Theme::default(),
        }
    }
}

impl Context {
    pub fn new() -> Self {
        Self {
            previous_searches: Vec::new(),
            clipboard: Clipboard::new(),
            clipboard_content: None,
            theme: Theme::default(),
        }
    }
    pub fn last_search(&self) -> Option<Search> {
        self.previous_searches.last().cloned()
    }

    pub fn set_search(&mut self, search: Search) {
        self.previous_searches.push(search)
    }

    pub fn previous_searches(&self) -> Vec<Search> {
        self.previous_searches.clone()
    }

    pub fn get_clipboard_content(&self) -> Option<String> {
        self.clipboard
            .get_content()
            .or_else(|| self.clipboard_content.clone())
    }

    pub fn set_clipboard_content(&mut self, content: String) {
        self.clipboard.set_content(content.clone());
        self.clipboard_content = Some(content);
    }
}
