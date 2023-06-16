use crate::{canonicalized_path::CanonicalizedPath, screen::RequestParams};
use std::{collections::HashMap, sync::mpsc::Sender};

use itertools::Itertools;

use crate::{lsp::language::get_languages, screen::ScreenMessage, utils::consolidate_errors};

use super::{language::Language, process::LspServerProcessChannel};

pub struct LspManager {
    lsp_server_process_channels: HashMap<Language, LspServerProcessChannel>,
    sender: Sender<ScreenMessage>,
}

impl Drop for LspManager {
    fn drop(&mut self) {
        for (_, channel) in self.lsp_server_process_channels.drain() {
            channel
                .shutdown()
                .unwrap_or_else(|error| log::error!("{:?}", error));
        }
    }
}

impl LspManager {
    pub fn new(clone: Sender<ScreenMessage>) -> LspManager {
        LspManager {
            lsp_server_process_channels: HashMap::new(),
            sender: clone,
        }
    }

    fn invoke_channels(
        &self,
        path: &CanonicalizedPath,
        error: &str,
        f: impl Fn(&LspServerProcessChannel) -> anyhow::Result<()>,
    ) -> anyhow::Result<()> {
        let languages = get_languages(path);
        let results = languages
            .into_iter()
            .filter_map(|language| self.lsp_server_process_channels.get(&language))
            .map(f)
            .collect_vec();
        consolidate_errors(error, results)
    }

    pub fn request_completion(&self, params: RequestParams) -> anyhow::Result<()> {
        self.invoke_channels(&params.path, "Failed to request completion", |channel| {
            channel.request_completion(params.clone())
        })
    }

    pub fn request_hover(&self, params: RequestParams) -> anyhow::Result<()> {
        self.invoke_channels(&params.path, "Failed to request hover", |channel| {
            channel.request_hover(params.clone())
        })
    }

    pub fn request_definition(&self, params: RequestParams) -> anyhow::Result<()> {
        self.invoke_channels(&params.path, "Failed to go to definition", |channel| {
            channel.request_definition(params.clone())
        })
    }

    pub fn request_references(&self, params: RequestParams) -> anyhow::Result<()> {
        self.invoke_channels(&params.path, "Failed to find references", |channel| {
            channel.request_references(params.clone())
        })
    }

    pub fn document_did_change(
        &self,
        path: CanonicalizedPath,
        content: String,
    ) -> anyhow::Result<()> {
        self.invoke_channels(&path, "Failed to notify document did change", |channel| {
            channel.document_did_change(&path, &content)
        })
    }

    pub fn document_did_save(&self, path: CanonicalizedPath) -> anyhow::Result<()> {
        self.invoke_channels(&path, "Failed to notify document did save", |channel| {
            channel.document_did_save(&path)
        })
    }

    /// Open file can do one of the following:
    /// 1. Start a new LSP server process if it is not started yet.
    /// 2. Notify the LSP server process that a new file is opened.
    /// 3. Do nothing if the LSP server process is spawned but not yet initialized.
    pub fn open_file(&mut self, path: CanonicalizedPath) -> Result<(), anyhow::Error> {
        let languages = get_languages(&path);

        consolidate_errors(
            "Failed to start language server",
            languages
                .into_iter()
                .map(|language| {
                    if let Some(channel) = self.lsp_server_process_channels.get(&language) {
                        if channel.is_initialized() {
                            channel.document_did_open(path.clone())
                        } else {
                            Ok(())
                        }
                    } else {
                        language.spawn_lsp(self.sender.clone()).map(|channel| {
                            self.lsp_server_process_channels.insert(language, channel);
                        })
                    }
                })
                .collect_vec(),
        )
    }

    pub fn initialized(&mut self, language: Language, opened_documents: Vec<CanonicalizedPath>) {
        self.lsp_server_process_channels
            .get_mut(&language)
            .map(|channel| {
                channel.initialized();
                channel.documents_did_open(opened_documents)
            });
    }
}
