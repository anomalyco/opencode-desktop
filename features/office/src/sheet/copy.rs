//! A copy of cells as they show: the document's thread writes it (`office_preview::xlsx::copy`).

/// The most cells one copy reads (`copyLimit`); a larger selection would hold the workbook's
/// thread for seconds.
pub const LIMIT: u64 = 200_000;
