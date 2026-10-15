//! Geometry, colour types, pixel formats, error types, and IDs shared by every crate.
//!
//! See PRD §7.2 for where this crate sits in the workspace layering, and
//! `docs/adr/` for the decisions that shape it. `aurora-core` depends on
//! nothing else in the workspace (`scripts/layering.json`) — it is the
//! foundation every other crate builds on, so it stays free of anything
//! crate-specific (no pixel storage, no tile concept, no document model).

pub mod black_white;
pub mod color;
pub mod color_balance;
pub mod curves;
pub mod effects;
pub mod error;
pub mod exposure;
pub mod geometry;
pub mod hue_ranges;
pub mod hue_saturation;
pub mod id;
pub mod levels;
pub mod photo_filter;
pub mod tone_curve;
pub mod vibrance;

pub use black_white::{
    BLACK_WHITE_DEFAULTS, BLACK_WHITE_MAX, BLACK_WHITE_MIN, BLACK_WHITE_NAMES, BlackWhiteError,
    BlackWhiteParams, DEFAULT_TINT_HUE, DEFAULT_TINT_SATURATION,
};
pub use color::{Channels, ColorSpace, PixelFormat, SampleFormat};
pub use color_balance::{
    COLOR_BALANCE_MAX, COLOR_BALANCE_RANGES, ColorBalanceError, ColorBalanceParams,
};
pub use curves::CurvesParams;
pub use effects::LayerEffects;
pub use error::CoreError;
pub use exposure::{EXPOSURE_MAX, ExposureError, ExposureParams, OFFSET_MAX};
pub use geometry::{MAX_DOCUMENT_EXTENT, MAX_DOCUMENT_ORIGIN, Rect, Size};
pub use hue_ranges::{
    DEFAULT_HUE_RANGE_EDGES, HUE_RANGE_COUNT, HUE_RANGE_NAMES, HueRange, HueRangeError,
    HueSaturationRangesParams, forward_gap,
};
pub use hue_saturation::{HueSaturationError, HueSaturationParams};
pub use id::{Id, IdGenerator};
pub use levels::{GAMMA_MAX, GAMMA_MIN, LevelsChannel, LevelsError, LevelsParams, MIN_INPUT_SPAN};
pub use photo_filter::{
    DEFAULT_DENSITY, PHOTO_FILTER_PRESETS, PhotoFilterError, PhotoFilterParams,
};
pub use tone_curve::{
    CurvePoint, MAX_LUT_LEN, MAX_POINTS, MIN_POINT_SEPARATION, MIN_POINTS, ToneCurve,
    ToneCurveError,
};
pub use vibrance::{VibranceError, VibranceParams};
