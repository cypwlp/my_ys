//! The façade the UI talks to: pick engines, run, and apply the policies that
//! must hold no matter which engine won.

use crate::cancel::Cancel;
use crate::codec_engine::CodecEngine;
use crate::engine::{with_fallback, Engine, EngineError, EngineResult, Op, Registry};
use crate::formats::Format;
use crate::models::{ArchiveInfo, CompressOptions, ExtractRequest, Outcome, Progress};
use crate::sevenzip::{self, SevenZip};
use crate::tar::TarEngine;
use crate::zip_engine::ZipEngine;
use std::path::Path;

pub struct Ops {
    registry: Registry,
}

/// One row of the settings pane's engine table.
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineReport {
    pub id: String,
    pub kind: String,
    pub formats: Vec<(String, bool)>,
    pub path: Option<String>,
}

impl Ops {
    /// Build the fleet from what is actually installed.  A missing 7-Zip is not an
    /// error: the managed engines still cover tar/zip, and the UI shows the gap.
    pub fn discover() -> Self {
        let mut engines: Vec<Box<dyn Engine>> = vec![Box::new(TarEngine), Box::new(ZipEngine), Box::new(CodecEngine)];
        if let Some(exe) = sevenzip::find_exe() {
            if let Ok(engine) = SevenZip::probe(exe) {
                engines.push(Box::new(engine));
            }
        }
        Self { registry: Registry::new(engines) }
    }

    pub fn with_registry(registry: Registry) -> Self {
        Self { registry }
    }

    pub fn engine_ids(&self) -> Vec<&'static str> {
        self.registry.ids()
    }

    pub fn fleet_report(&self) -> Vec<EngineReport> {
        self.registry
            .engines()
            .iter()
            .map(|e| EngineReport {
                id: e.id().to_string(),
                kind: format!("{:?}", e.kind()),
                formats: e.matrix().into_iter().map(|(f, w)| (f.to_string(), w)).collect(),
                path: e.location(),
            })
            .collect()
    }

    pub fn candidates(&self, format: Format, op: Op) -> Vec<&dyn Engine> {
        self.registry.candidates(format, op)
    }

    pub fn list(&self, archive: &Path, password: Option<&str>) -> EngineResult<ArchiveInfo> {
        let format = crate::formats::sniff(archive);
        let candidates = self.candidates(format, Op::Read);
        with_fallback(&candidates, |engine| engine.list(archive, password)).map(|mut info| {
            info.format = format;
            info
        })
    }

    pub fn extract(
        &self,
        req: &ExtractRequest,
        report: &mut dyn FnMut(Progress),
        cancel: Cancel,
    ) -> EngineResult<Outcome> {
        let format = crate::formats::sniff(Path::new(&req.archive));
        let candidates = self.candidates(format, Op::Read);
        let mut outcome = with_fallback(&candidates, |engine| engine.extract_cancellable(req, report, cancel))?;
        // "0 items extracted" with an explicit member list is a misspelled filter,
        // not a success.  Silence here once cost a whole debugging session.
        if outcome.ok && !req.members.is_empty() && outcome.items == 0 {
            outcome.fail(format!(
                "壓縮包內找不到這些條目（指定 {} 項）",
                req.members.len()
            ));
        }
        Ok(outcome)
    }

    pub fn create(
        &self,
        opts: &CompressOptions,
        report: &mut dyn FnMut(Progress),
        cancel: Cancel,
    ) -> EngineResult<Outcome> {
        let candidates = self.candidates(opts.format, Op::Write);
        if candidates.is_empty() {
            // Measured, not assumed: 7zr 26.00 rejects `-ttxz` outright, and given a
            // `*.tar.xz` target without `-t` it writes a *plain xz* of the files --
            // no tar header at all.  Silently handing the user a mis-named archive is
            // worse than refusing, so say what to use instead.
            if opts.format == Format::TarXz {
                return Err(EngineError::UnsupportedFormat(
                    "tar.xz 沒有一步完成的寫入引擎（7z 的 -ttxz 被拒，改檔名它只會寫成純 xz）；請用 tar.zst、tar.gz 或 7z".into(),
                ));
            }
            return Err(EngineError::UnsupportedFormat(opts.format.to_string()));
        }
        with_fallback(&candidates, |engine| engine.create_cancellable(opts, report, cancel))
    }
}
