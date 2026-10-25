//! Plugs `aurora-app`'s headless document session into `aurora-mcp`'s
//! [`Session`] trait. A newtype, because the trait (`aurora-mcp`) and the
//! type (`aurora-app`) are both foreign here; the methods only translate
//! types, they add no behaviour.

use std::path::Path;

use aurora_app::mcp_session::{ExportKind, ExportResult, Failure, HeadlessSession, SessionLimits};
use aurora_commands::{CommandContext, Validated};
use aurora_mcp::{Cancel, ExportFormat, ExportOutcome, Preview, Session, SessionError};
use serde_json::Value;

/// The host `aurora-cli mcp` serves.
#[derive(Debug, Default)]
pub struct AppHost(HeadlessSession);

impl AppHost {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A host under the server's limits, so the session's composite and
    /// export bounds match what the server checks.
    #[must_use]
    pub fn with_limits(limits: &aurora_mcp::Limits) -> Self {
        Self(HeadlessSession::with_limits(SessionLimits {
            max_pixels: limits.max_open_pixels,
            max_export_bytes: limits.max_export_bytes,
        }))
    }
}

fn kind_of(format: ExportFormat) -> ExportKind {
    match format {
        ExportFormat::Png => ExportKind::Png,
        ExportFormat::Jpeg => ExportKind::Jpeg,
        ExportFormat::Tiff => ExportKind::Tiff,
        ExportFormat::Psd => ExportKind::Psd,
        ExportFormat::Aur => ExportKind::Aur,
    }
}

fn error(failure: Failure) -> SessionError {
    match failure {
        Failure::NoDocument => SessionError::NoDocument,
        Failure::NotAvailable(m) => SessionError::NotAvailable(m),
        Failure::Refused(m) => SessionError::Refused(m),
        Failure::Failed(m) => SessionError::Failed(m),
        Failure::Cancelled => SessionError::Cancelled,
    }
}

impl Session for AppHost {
    fn set_scratch_dir(&mut self, dir: &Path) {
        self.0.set_scratch_dir(dir);
    }

    fn open(
        &mut self,
        staged: &Path,
        name: &str,
        max_pixels: u64,
        cancel: &Cancel,
    ) -> Result<Value, SessionError> {
        if cancel.is_cancelled() {
            return Err(SessionError::Cancelled);
        }
        self.0.open(staged, name, max_pixels).map_err(error)
    }

    fn export_estimate(&mut self, format: ExportFormat) -> Option<u64> {
        self.0.export_estimate(kind_of(format))
    }

    fn inspect(&mut self) -> Result<Value, SessionError> {
        self.0.inspect().map_err(error)
    }

    fn has_document(&self) -> bool {
        self.0.has_document()
    }

    fn command_context(&mut self) -> CommandContext {
        self.0.command_context()
    }

    fn headless_commands(&self) -> &'static [&'static str] {
        self.0.headless_commands()
    }

    fn run_command(&mut self, call: &Validated) -> Result<Value, SessionError> {
        self.0.run_command(call).map_err(error)
    }

    fn preview(&mut self, max_side: u32, cancel: &Cancel) -> Result<Preview, SessionError> {
        let p = self
            .0
            .preview(max_side, &|| cancel.is_cancelled())
            .map_err(error)?;
        Ok(Preview {
            png: p.png,
            width: p.width,
            height: p.height,
            document_width: p.document_width,
            document_height: p.document_height,
        })
    }

    fn export(
        &mut self,
        format: ExportFormat,
        accept_losses: bool,
        out: &Path,
        cancel: &Cancel,
        progress: &mut dyn FnMut(f64),
    ) -> Result<ExportOutcome, SessionError> {
        let kind = kind_of(format);
        match self
            .0
            .export(
                kind,
                accept_losses,
                out,
                &|| cancel.is_cancelled(),
                progress,
            )
            .map_err(error)?
        {
            ExportResult::Written(report) => Ok(ExportOutcome::Written { report }),
            ExportResult::NeedsConfirmation(report) => {
                Ok(ExportOutcome::NeedsConfirmation { report })
            }
        }
    }

    fn psd_check(&mut self, cancel: &Cancel) -> Result<Value, SessionError> {
        self.0.psd_check(&|| cancel.is_cancelled()).map_err(error)
    }
}
