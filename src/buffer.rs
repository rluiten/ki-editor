use std::{ops::Range, path::Path};

use ropey::Rope;
use tree_sitter::{InputEdit, Node, Parser, Point, Tree};
use tree_sitter_traversal::{traverse, Order};

use crate::{
    edit::{Edit, EditTransaction},
    engine::EditHistoryKind,
    selection::{CharIndex, Selection, SelectionSet, ToRangeUsize},
    utils::find_previous,
};

#[derive(Clone)]
pub struct Buffer {
    rope: Rope,
    tree: Tree,
    undo_patches: Vec<Patch>,
    redo_patches: Vec<Patch>,
}

impl Buffer {
    pub fn new(language: tree_sitter::Language, text: &str) -> Self {
        Self {
            rope: Rope::from_str(text),
            tree: {
                let mut parser = Parser::new();
                parser.set_language(language).unwrap();
                parser.parse(text.to_string(), None).unwrap()
            },
            undo_patches: Vec::new(),
            redo_patches: Vec::new(),
        }
    }

    pub fn get_line(&self, char_index: CharIndex) -> String {
        let line = self.rope.line(self.char_to_line(char_index));
        line.to_string()
    }

    pub fn char_to_line(&self, char_index: CharIndex) -> usize {
        self.rope.char_to_line(char_index.0)
    }

    pub fn line_to_char(&self, line_index: usize) -> CharIndex {
        CharIndex(self.rope.line_to_char(line_index))
    }

    pub fn char_to_byte(&self, char_index: CharIndex) -> usize {
        self.rope.char_to_byte(char_index.0)
    }

    pub fn char_to_point(&self, char_index: CharIndex) -> tree_sitter::Point {
        let line = self.char_to_line(char_index);
        Point {
            row: line,
            column: self
                .rope
                .try_line_to_char(line)
                .map(|line_start_char_index| char_index.0.saturating_sub(line_start_char_index))
                .unwrap_or(0),
        }
    }

    pub fn byte_to_char(&self, byte_index: usize) -> CharIndex {
        CharIndex(self.rope.byte_to_char(byte_index))
    }

    pub fn rope(&self) -> &Rope {
        &self.rope
    }

    pub fn len_chars(&self) -> usize {
        self.rope.len_chars()
    }

    pub fn tree(&self) -> &Tree {
        &self.tree
    }

    pub fn slice(&self, range: &Range<CharIndex>) -> Rope {
        self.rope.slice(range.to_usize_range()).into()
    }

    pub fn get_nearest_node_after_char(&self, char_index: CharIndex) -> Option<Node> {
        let byte = self.char_to_byte(char_index);
        // Preorder is the main key here,
        // because preorder traversal walks the parent first
        traverse(self.tree.root_node().walk(), Order::Pre).find(|&node| node.start_byte() >= byte)
    }

    pub fn get_current_node<'a>(
        &'a self,
        cursor_char_index: CharIndex,
        selection: &Selection,
    ) -> Node<'a> {
        if let Some(node_id) = selection.node_id {
            self.get_node_by_id(node_id)
        } else {
            self.get_nearest_node_after_char(cursor_char_index)
        }
        // TODO: should not return root node if not found
        .unwrap_or_else(|| self.tree.root_node())
    }

    pub fn get_next_token(&self, char_index: CharIndex, is_named: bool) -> Option<Node> {
        let byte = self.char_to_byte(char_index);
        self.traverse(Order::Post).find(|&node| {
            node.child_count() == 0 && (!is_named || node.is_named()) && node.end_byte() > byte
        })
    }

    pub fn get_prev_token(&self, char_index: CharIndex, is_named: bool) -> Option<Node> {
        let byte = self.char_to_byte(char_index);
        find_previous(
            self.traverse(Order::Post),
            |_, _| true,
            |node| {
                node.child_count() == 0
                    && (!is_named || node.is_named())
                    && node.start_byte() >= byte
            },
        )
    }

    fn get_node_by_id(&self, node_id: usize) -> Option<Node> {
        traverse(self.tree.walk(), Order::Pre).find(|node| node.id() == node_id)
    }

    pub fn traverse(&self, order: Order) -> impl Iterator<Item = Node> {
        traverse(self.tree.walk(), order)
    }

    pub fn apply_edit_transaction(
        &mut self,
        edit_transaction: &EditTransaction,
        current_selection_set: SelectionSet,
        edit_history_kind: EditHistoryKind,
    ) -> Result<(), anyhow::Error> {
        edit_transaction
            .edits()
            .into_iter()
            .fold(Ok(()), |result, edit| match result {
                Err(err) => Err(err),
                Ok(()) => self.apply_edit(&edit),
            })?;

        let patch = Patch {
            selection_set: current_selection_set,
            edit_transaction: edit_transaction.inverse(),
        };

        match edit_history_kind {
            EditHistoryKind::NewEdit => {
                self.redo_patches.clear();
                self.undo_patches.push(patch);
            }
            EditHistoryKind::Undo => {
                self.redo_patches.push(patch);
            }
            EditHistoryKind::Redo => {
                self.undo_patches.push(patch);
            }
        }

        Ok(())
    }

    pub fn undo(&mut self, current_selection_set: SelectionSet) -> Option<SelectionSet> {
        if let Some(patch) = self.undo_patches.pop() {
            self.revert_change(&patch, current_selection_set, EditHistoryKind::Undo);
            Some(patch.selection_set)
        } else {
            log::info!("Nothing else to be undone");
            None
        }
    }

    pub fn redo(&mut self, current_selection_set: SelectionSet) -> Option<SelectionSet> {
        if let Some(patch) = self.redo_patches.pop() {
            self.revert_change(&patch, current_selection_set, EditHistoryKind::Redo);
            Some(patch.selection_set)
        } else {
            log::info!("Nothing else to be redone");
            None
        }
    }

    fn revert_change(
        &mut self,
        patch: &Patch,
        current_selection_set: SelectionSet,
        edit_history_kind: EditHistoryKind,
    ) {
        self.apply_edit_transaction(
            &patch.edit_transaction,
            current_selection_set,
            edit_history_kind,
        )
        .unwrap();
    }

    pub fn apply_edit(&mut self, edit: &Edit) -> Result<(), anyhow::Error> {
        let start_char_index = edit.start;
        let old_end_char_index = edit.end();
        let new_end_char_index = edit.start + edit.new.len_chars();

        let start_byte = self.char_to_byte(start_char_index);
        let old_end_byte = self.char_to_byte(old_end_char_index);
        let start_position = self.char_to_point(start_char_index);
        let old_end_position = self.char_to_point(old_end_char_index);

        self.rope.remove(edit.start.0..edit.end().0);
        self.rope
            .insert(edit.start.0, edit.new.to_string().as_str());

        let new_end_byte = self.char_to_byte(new_end_char_index);
        let new_end_position = self.char_to_point(new_end_char_index);

        let mut parser = tree_sitter::Parser::new();
        parser.set_language(self.tree.language()).unwrap();
        self.tree.edit(&InputEdit {
            start_byte,
            old_end_byte,
            new_end_byte,
            start_position,
            old_end_position,
            new_end_position,
        });

        self.tree = parser
            .parse(&self.rope.to_string(), Some(&self.tree))
            .unwrap();

        Ok(())
    }

    pub fn has_syntax_error_at(&self, range: Range<CharIndex>) -> bool {
        let rope = &self.rope;
        if let Some(node) = self.tree.root_node().descendant_for_byte_range(
            rope.try_char_to_byte(range.start.0).unwrap_or(0),
            rope.try_char_to_byte(range.end.0).unwrap_or(0),
        ) {
            node.has_error()
        } else {
            false
        }
    }

    pub fn from_path(path: &Path) -> Buffer {
        let content = std::fs::read_to_string(path).unwrap();
        let language = match path.extension().unwrap().to_str().unwrap() {
            "js" | "jsx" => tree_sitter_javascript::language(),
            "ts" => tree_sitter_typescript::language_typescript(),
            "tsx" => tree_sitter_typescript::language_tsx(),
            "rs" => tree_sitter_rust::language(),
            "md" => tree_sitter_md::language(),
            _ => panic!("Unsupported file extension"),
        };

        Buffer::new(language, &content)
    }
}

#[derive(Clone, Debug)]
pub struct Patch {
    pub edit_transaction: EditTransaction,
    /// Used for restoring previous selection after undo/redo
    pub selection_set: SelectionSet,
}
