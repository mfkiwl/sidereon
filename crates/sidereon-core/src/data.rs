//! GNSS product filename and archive URL catalog.
//!
//! This module is sans-IO: it performs no network access, reads no files, and
//! writes no cache entries. It only turns cataloged center/product/date inputs
//! into canonical archive filenames and URLs.

use core::fmt;
use core::str::FromStr;

use crate::astro::time::civil::{civil_from_julian_day_number, day_of_year_int, days_in_month};
use crate::astro::time::gnss::{week_epoch_julian_day_number, week_from_calendar};
use crate::astro::time::model::TimeScale;
use crate::astro::time::scales::julian_day_number;

/// Analysis-center code supported by the data-product catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum AnalysisCenter {
    /// `igs`.
    Igs,
    /// `cod_rap`.
    CodRap,
    /// `cod_prd1`.
    CodPrd1,
    /// `cod_prd2`.
    CodPrd2,
    /// `esa`.
    Esa,
    /// `cod`.
    Cod,
    /// `gfz`.
    Gfz,
    /// `igs_ult`.
    IgsUlt,
    /// `cod_ult`.
    CodUlt,
    /// `esa_ult`.
    EsaUlt,
    /// `gfz_ult`.
    GfzUlt,
}

impl AnalysisCenter {
    /// The lower-case catalog code.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Igs => "igs",
            Self::CodRap => "cod_rap",
            Self::CodPrd1 => "cod_prd1",
            Self::CodPrd2 => "cod_prd2",
            Self::Esa => "esa",
            Self::Cod => "cod",
            Self::Gfz => "gfz",
            Self::IgsUlt => "igs_ult",
            Self::CodUlt => "cod_ult",
            Self::EsaUlt => "esa_ult",
            Self::GfzUlt => "gfz_ult",
        }
    }

    /// Parse a lower-case catalog code.
    #[must_use]
    pub fn from_code(code: &str) -> Option<Self> {
        match code {
            "igs" => Some(Self::Igs),
            "cod_rap" => Some(Self::CodRap),
            "cod_prd1" => Some(Self::CodPrd1),
            "cod_prd2" => Some(Self::CodPrd2),
            "esa" => Some(Self::Esa),
            "cod" => Some(Self::Cod),
            "gfz" => Some(Self::Gfz),
            "igs_ult" => Some(Self::IgsUlt),
            "cod_ult" => Some(Self::CodUlt),
            "esa_ult" => Some(Self::EsaUlt),
            "gfz_ult" => Some(Self::GfzUlt),
            _ => None,
        }
    }
}

impl fmt::Display for AnalysisCenter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

impl FromStr for AnalysisCenter {
    type Err = DataCatalogError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_code(s).ok_or_else(|| DataCatalogError::UnknownCenter(s.to_string()))
    }
}

/// Product type supported by the data-product catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ProductType {
    /// Precise orbit SP3.
    Sp3,
    /// RINEX clock.
    Clk,
    /// Merged broadcast navigation.
    Nav,
    /// IONEX global ionosphere map.
    Ionex,
}

impl ProductType {
    /// The lower-case product code.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Sp3 => "sp3",
            Self::Clk => "clk",
            Self::Nav => "nav",
            Self::Ionex => "ionex",
        }
    }

    /// Parse a lower-case product code.
    #[must_use]
    pub fn from_code(code: &str) -> Option<Self> {
        match code {
            "sp3" => Some(Self::Sp3),
            "clk" => Some(Self::Clk),
            "nav" => Some(Self::Nav),
            "ionex" => Some(Self::Ionex),
            _ => None,
        }
    }
}

impl fmt::Display for ProductType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

impl FromStr for ProductType {
    type Err = DataCatalogError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_code(s).ok_or_else(|| DataCatalogError::UnknownProductType(s.to_string()))
    }
}

/// Archive transport protocol recorded by the catalog.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ArchiveProtocol {
    /// HTTP.
    Http,
    /// HTTPS.
    Https,
}

impl ArchiveProtocol {
    /// URI scheme text.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Http => "http",
            Self::Https => "https",
        }
    }
}

/// Archive compression for a cataloged product.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ArchiveCompression {
    /// Archive URL has a `.gz` suffix.
    Gzip,
    /// Archive URL is the plain product filename.
    None,
}

impl ArchiveCompression {
    const fn suffix(self) -> &'static str {
        match self {
            Self::Gzip => ".gz",
            Self::None => "",
        }
    }
}

/// Directory layout used below an archive root.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ArchiveLayout {
    /// `rapid/w<gps-week>`.
    GfzRapidWeek,
    /// `ultra/w<gps-week>`.
    GfzUltraWeek,
    /// `<gps-week>`.
    GpsWeek,
    /// `products/<gps-week>`.
    BkgProductsWeek,
    /// `BRDC/<year>/<day-of-year>`.
    BkgBrdcYearDoy,
    /// `obs/<year>/<day-of-year>`.
    BkgObsYearDoy,
    /// `CODE_MGEX/CODE/<year>`.
    AiubCodeMgexYear,
    /// `CODE/<year>`.
    AiubCodeYear,
    /// `CODE`.
    AiubCodeRoot,
}

/// Product filename convention.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProductFilenameKind {
    /// `TOKEN_DATE_LEN_SAMPLE_CODE.EXT`.
    Sampled,
    /// `TOKEN_R_DATE_LEN_CODE.ext`.
    Nav,
}

/// Product-type filename convention.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProductTypeConvention {
    /// Product type.
    pub product_type: ProductType,
    /// Filename content code, for example `ORB`.
    pub content_code: &'static str,
    /// Filename extension, preserving archive case.
    pub extension: &'static str,
    /// Filename convention.
    pub kind: ProductFilenameKind,
}

/// Per-center convention for one product type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CenterProductConvention {
    /// Product type.
    pub product_type: ProductType,
    /// IGS long-name token prefix.
    pub token: &'static str,
    /// Directory layout under the archive root.
    pub layout: ArchiveLayout,
    /// Product span token.
    pub span: &'static str,
    /// Default sampling token.
    pub default_sample: &'static str,
    /// Archive compression.
    pub compression: ArchiveCompression,
}

/// Static catalog entry for one analysis-center code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CenterCatalogEntry {
    /// Analysis-center code.
    pub center: AnalysisCenter,
    /// Lower-case catalog code.
    pub code: &'static str,
    /// Archive URI scheme.
    pub protocol: ArchiveProtocol,
    /// Archive host.
    pub host: &'static str,
    /// Archive root URL without trailing slash.
    pub root_url: &'static str,
    /// Product conventions served by this center.
    pub products: &'static [CenterProductConvention],
    /// Valid issue times for sub-daily products.
    pub issues: &'static [&'static str],
}

/// Product pair that is intentionally not offered because no open mirror exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NoOpenMirrorProduct {
    /// Analysis-center code.
    pub center: &'static str,
    /// Product type code.
    pub product_type: &'static str,
}

const PRODUCT_TYPE_CONVENTIONS: [ProductTypeConvention; 4] = [
    ProductTypeConvention {
        product_type: ProductType::Sp3,
        content_code: "ORB",
        extension: "SP3",
        kind: ProductFilenameKind::Sampled,
    },
    ProductTypeConvention {
        product_type: ProductType::Clk,
        content_code: "CLK",
        extension: "CLK",
        kind: ProductFilenameKind::Sampled,
    },
    ProductTypeConvention {
        product_type: ProductType::Nav,
        content_code: "MN",
        extension: "rnx",
        kind: ProductFilenameKind::Nav,
    },
    ProductTypeConvention {
        product_type: ProductType::Ionex,
        content_code: "GIM",
        extension: "INX",
        kind: ProductFilenameKind::Sampled,
    },
];

const COD_RAP_PRODUCTS: [CenterProductConvention; 1] = [CenterProductConvention {
    product_type: ProductType::Ionex,
    token: "COD0OPSRAP",
    layout: ArchiveLayout::AiubCodeRoot,
    span: "01D",
    default_sample: "01H",
    compression: ArchiveCompression::Gzip,
}];

const COD_PRD_PRODUCTS: [CenterProductConvention; 1] = [CenterProductConvention {
    product_type: ProductType::Ionex,
    token: "COD0OPSPRD",
    layout: ArchiveLayout::AiubCodeRoot,
    span: "01D",
    default_sample: "01H",
    compression: ArchiveCompression::Gzip,
}];

const ESA_PRODUCTS: [CenterProductConvention; 3] = [
    CenterProductConvention {
        product_type: ProductType::Sp3,
        token: "ESA0MGNFIN",
        layout: ArchiveLayout::GpsWeek,
        span: "01D",
        default_sample: "05M",
        compression: ArchiveCompression::Gzip,
    },
    CenterProductConvention {
        product_type: ProductType::Clk,
        token: "ESA0MGNFIN",
        layout: ArchiveLayout::GpsWeek,
        span: "01D",
        default_sample: "30S",
        compression: ArchiveCompression::Gzip,
    },
    CenterProductConvention {
        product_type: ProductType::Ionex,
        token: "ESA0OPSFIN",
        layout: ArchiveLayout::GpsWeek,
        span: "01D",
        default_sample: "02H",
        compression: ArchiveCompression::Gzip,
    },
];

const COD_PRODUCTS: [CenterProductConvention; 3] = [
    CenterProductConvention {
        product_type: ProductType::Sp3,
        token: "COD0MGXFIN",
        layout: ArchiveLayout::AiubCodeMgexYear,
        span: "01D",
        default_sample: "05M",
        compression: ArchiveCompression::Gzip,
    },
    CenterProductConvention {
        product_type: ProductType::Clk,
        token: "COD0MGXFIN",
        layout: ArchiveLayout::AiubCodeMgexYear,
        span: "01D",
        default_sample: "30S",
        compression: ArchiveCompression::Gzip,
    },
    CenterProductConvention {
        product_type: ProductType::Ionex,
        token: "COD0OPSFIN",
        layout: ArchiveLayout::AiubCodeYear,
        span: "01D",
        default_sample: "01H",
        compression: ArchiveCompression::Gzip,
    },
];

const GFZ_PRODUCTS: [CenterProductConvention; 2] = [
    CenterProductConvention {
        product_type: ProductType::Sp3,
        token: "GFZ0OPSRAP",
        layout: ArchiveLayout::GfzRapidWeek,
        span: "01D",
        default_sample: "15M",
        compression: ArchiveCompression::Gzip,
    },
    CenterProductConvention {
        product_type: ProductType::Clk,
        token: "GFZ0OPSRAP",
        layout: ArchiveLayout::GfzRapidWeek,
        span: "01D",
        default_sample: "30S",
        compression: ArchiveCompression::Gzip,
    },
];

const IGS_PRODUCTS: [CenterProductConvention; 1] = [CenterProductConvention {
    product_type: ProductType::Nav,
    token: "BRDC00WRD",
    layout: ArchiveLayout::BkgBrdcYearDoy,
    span: "01D",
    default_sample: "01D",
    compression: ArchiveCompression::Gzip,
}];

const IGS_ULT_PRODUCTS: [CenterProductConvention; 1] = [CenterProductConvention {
    product_type: ProductType::Sp3,
    token: "IGS0OPSULT",
    layout: ArchiveLayout::BkgProductsWeek,
    span: "02D",
    default_sample: "15M",
    compression: ArchiveCompression::Gzip,
}];

const COD_ULT_PRODUCTS: [CenterProductConvention; 1] = [CenterProductConvention {
    product_type: ProductType::Sp3,
    token: "COD0OPSULT",
    layout: ArchiveLayout::AiubCodeRoot,
    span: "01D",
    default_sample: "05M",
    compression: ArchiveCompression::None,
}];

const ESA_ULT_PRODUCTS: [CenterProductConvention; 1] = [CenterProductConvention {
    product_type: ProductType::Sp3,
    token: "ESA0OPSULT",
    layout: ArchiveLayout::GpsWeek,
    span: "02D",
    default_sample: "15M",
    compression: ArchiveCompression::Gzip,
}];

const GFZ_ULT_PRODUCTS: [CenterProductConvention; 1] = [CenterProductConvention {
    product_type: ProductType::Sp3,
    token: "GFZ0OPSULT",
    layout: ArchiveLayout::GfzUltraWeek,
    span: "02D",
    default_sample: "05M",
    compression: ArchiveCompression::Gzip,
}];

const OPSULT_ISSUES: [&str; 4] = ["0000", "0600", "1200", "1800"];
const COD_ULT_ISSUES: [&str; 1] = ["0000"];

const CENTER_ORDER: [AnalysisCenter; 11] = [
    AnalysisCenter::CodRap,
    AnalysisCenter::CodPrd1,
    AnalysisCenter::CodPrd2,
    AnalysisCenter::Igs,
    AnalysisCenter::Esa,
    AnalysisCenter::Cod,
    AnalysisCenter::Gfz,
    AnalysisCenter::IgsUlt,
    AnalysisCenter::CodUlt,
    AnalysisCenter::EsaUlt,
    AnalysisCenter::GfzUlt,
];

const CATALOG: [CenterCatalogEntry; 11] = [
    CenterCatalogEntry {
        center: AnalysisCenter::CodRap,
        code: "cod_rap",
        protocol: ArchiveProtocol::Http,
        host: "ftp.aiub.unibe.ch",
        root_url: "http://ftp.aiub.unibe.ch",
        products: &COD_RAP_PRODUCTS,
        issues: &[],
    },
    CenterCatalogEntry {
        center: AnalysisCenter::CodPrd1,
        code: "cod_prd1",
        protocol: ArchiveProtocol::Http,
        host: "ftp.aiub.unibe.ch",
        root_url: "http://ftp.aiub.unibe.ch",
        products: &COD_PRD_PRODUCTS,
        issues: &[],
    },
    CenterCatalogEntry {
        center: AnalysisCenter::CodPrd2,
        code: "cod_prd2",
        protocol: ArchiveProtocol::Http,
        host: "ftp.aiub.unibe.ch",
        root_url: "http://ftp.aiub.unibe.ch",
        products: &COD_PRD_PRODUCTS,
        issues: &[],
    },
    CenterCatalogEntry {
        center: AnalysisCenter::Igs,
        code: "igs",
        protocol: ArchiveProtocol::Https,
        host: "igs.bkg.bund.de",
        root_url: "https://igs.bkg.bund.de/root_ftp/IGS",
        products: &IGS_PRODUCTS,
        issues: &[],
    },
    CenterCatalogEntry {
        center: AnalysisCenter::Esa,
        code: "esa",
        protocol: ArchiveProtocol::Https,
        host: "navigation-office.esa.int",
        root_url: "https://navigation-office.esa.int/products/gnss-products",
        products: &ESA_PRODUCTS,
        issues: &[],
    },
    CenterCatalogEntry {
        center: AnalysisCenter::Cod,
        code: "cod",
        protocol: ArchiveProtocol::Http,
        host: "ftp.aiub.unibe.ch",
        root_url: "http://ftp.aiub.unibe.ch",
        products: &COD_PRODUCTS,
        issues: &[],
    },
    CenterCatalogEntry {
        center: AnalysisCenter::Gfz,
        code: "gfz",
        protocol: ArchiveProtocol::Https,
        host: "isdc-data.gfz.de",
        root_url: "https://isdc-data.gfz.de/gnss/products",
        products: &GFZ_PRODUCTS,
        issues: &[],
    },
    CenterCatalogEntry {
        center: AnalysisCenter::IgsUlt,
        code: "igs_ult",
        protocol: ArchiveProtocol::Https,
        host: "igs.bkg.bund.de",
        root_url: "https://igs.bkg.bund.de/root_ftp/IGS",
        products: &IGS_ULT_PRODUCTS,
        issues: &OPSULT_ISSUES,
    },
    CenterCatalogEntry {
        center: AnalysisCenter::CodUlt,
        code: "cod_ult",
        protocol: ArchiveProtocol::Http,
        host: "ftp.aiub.unibe.ch",
        root_url: "http://ftp.aiub.unibe.ch",
        products: &COD_ULT_PRODUCTS,
        issues: &COD_ULT_ISSUES,
    },
    CenterCatalogEntry {
        center: AnalysisCenter::EsaUlt,
        code: "esa_ult",
        protocol: ArchiveProtocol::Https,
        host: "navigation-office.esa.int",
        root_url: "https://navigation-office.esa.int/products/gnss-products",
        products: &ESA_ULT_PRODUCTS,
        issues: &OPSULT_ISSUES,
    },
    CenterCatalogEntry {
        center: AnalysisCenter::GfzUlt,
        code: "gfz_ult",
        protocol: ArchiveProtocol::Https,
        host: "isdc-data.gfz.de",
        root_url: "https://isdc-data.gfz.de/gnss/products",
        products: &GFZ_ULT_PRODUCTS,
        issues: &OPSULT_ISSUES,
    },
];

const ALLOWED_HOSTS: [&str; 4] = [
    "ftp.aiub.unibe.ch",
    "navigation-office.esa.int",
    "isdc-data.gfz.de",
    "igs.bkg.bund.de",
];

const NO_OPEN_MIRRORS: [NoOpenMirrorProduct; 7] = [
    NoOpenMirrorProduct {
        center: "grg",
        product_type: "sp3",
    },
    NoOpenMirrorProduct {
        center: "grg",
        product_type: "clk",
    },
    NoOpenMirrorProduct {
        center: "wum",
        product_type: "sp3",
    },
    NoOpenMirrorProduct {
        center: "wum",
        product_type: "clk",
    },
    NoOpenMirrorProduct {
        center: "grg_ult",
        product_type: "sp3",
    },
    NoOpenMirrorProduct {
        center: "grg_ult",
        product_type: "clk",
    },
    NoOpenMirrorProduct {
        center: "igs",
        product_type: "ionex",
    },
];

/// Error returned by the pure data-product catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DataCatalogError {
    /// Unknown analysis-center code.
    UnknownCenter(String),
    /// Unknown product type code.
    UnknownProductType(String),
    /// The center does not serve the requested product type.
    UnsupportedProduct {
        /// Analysis center.
        center: AnalysisCenter,
        /// Product type.
        product_type: ProductType,
    },
    /// The product has no verified anonymous HTTP(S) mirror.
    NoOpenMirror {
        /// Analysis-center code.
        center: String,
        /// Product type code.
        product_type: String,
    },
    /// Bad civil date.
    InvalidDate {
        /// Year.
        year: i32,
        /// Month.
        month: u8,
        /// Day.
        day: u8,
    },
    /// Date cannot be represented by this API.
    DateOutOfRange,
    /// Date precedes the GPS week epoch.
    DateBeforeGpsEpoch(ProductDate),
    /// GPS day-of-week must be `0..=6`.
    InvalidGpsDayOfWeek(u8),
    /// Sampling token is not `NNX` with an upper-case unit.
    InvalidSample(String),
    /// Issue time is malformed.
    InvalidIssue(String),
    /// The center requires an issue time.
    MissingIssue {
        /// Analysis center.
        center: AnalysisCenter,
    },
    /// The center does not use issue times.
    UnexpectedIssue {
        /// Analysis center.
        center: AnalysisCenter,
    },
    /// Issue time is valid text but not published by this center.
    UnsupportedIssue {
        /// Analysis center.
        center: AnalysisCenter,
        /// Issue time.
        issue: String,
    },
    /// The target datetime was invalid.
    InvalidDateTime {
        /// Hour.
        hour: u8,
        /// Minute.
        minute: u8,
        /// Second.
        second: u8,
    },
    /// No ultra-rapid issue exists at or before the requested target.
    NoUltraIssue,
    /// No available ultra-rapid issue exists at or before the requested target.
    NoAvailableUltraIssue,
    /// Station identifier is not a 9-character upper-case alphanumeric token.
    InvalidStation(String),
}

impl fmt::Display for DataCatalogError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownCenter(center) => write!(f, "unknown analysis center {center:?}"),
            Self::UnknownProductType(product_type) => {
                write!(f, "unknown product type {product_type:?}")
            }
            Self::UnsupportedProduct {
                center,
                product_type,
            } => write!(f, "{center} does not serve {product_type}"),
            Self::NoOpenMirror {
                center,
                product_type,
            } => write!(f, "{center}/{product_type} has no open mirror"),
            Self::InvalidDate { year, month, day } => {
                write!(f, "invalid product date {year:04}-{month:02}-{day:02}")
            }
            Self::DateOutOfRange => write!(f, "product date is out of range"),
            Self::DateBeforeGpsEpoch(date) => {
                write!(f, "product date {date} is before the GPS week epoch")
            }
            Self::InvalidGpsDayOfWeek(day) => {
                write!(f, "invalid GPS day-of-week {day}")
            }
            Self::InvalidSample(sample) => write!(f, "invalid sample code {sample:?}"),
            Self::InvalidIssue(issue) => write!(f, "invalid issue time {issue:?}"),
            Self::MissingIssue { center } => write!(f, "{center} requires an issue time"),
            Self::UnexpectedIssue { center } => write!(f, "{center} does not take an issue time"),
            Self::UnsupportedIssue { center, issue } => {
                write!(f, "{center} does not publish issue {issue:?}")
            }
            Self::InvalidDateTime {
                hour,
                minute,
                second,
            } => write!(f, "invalid product time {hour:02}:{minute:02}:{second:02}"),
            Self::NoUltraIssue => write!(f, "no ultra-rapid issue at or before target"),
            Self::NoAvailableUltraIssue => {
                write!(f, "no available ultra-rapid issue at or before target")
            }
            Self::InvalidStation(station) => write!(f, "invalid station code {station:?}"),
        }
    }
}

impl std::error::Error for DataCatalogError {}

/// Civil UTC date used by product archive names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ProductDate {
    /// Year.
    pub year: i32,
    /// Month in `1..=12`.
    pub month: u8,
    /// Day of month.
    pub day: u8,
}

impl ProductDate {
    /// Build and validate a civil date.
    pub fn new(year: i32, month: u8, day: u8) -> Result<Self, DataCatalogError> {
        let days = days_in_month(i64::from(year), i64::from(month));
        if !(1..=9999).contains(&year) || days == 0 || day == 0 || i64::from(day) > days {
            return Err(DataCatalogError::InvalidDate { year, month, day });
        }
        Ok(Self { year, month, day })
    }

    /// Build a date from GPS week and day-of-week (`0` = Sunday).
    pub fn from_gps_week_day(week: u32, day_of_week: u8) -> Result<Self, DataCatalogError> {
        if day_of_week > 6 {
            return Err(DataCatalogError::InvalidGpsDayOfWeek(day_of_week));
        }
        let epoch_jdn =
            week_epoch_julian_day_number(TimeScale::Gpst).expect("GPST has a week-numbering epoch");
        let offset_days = i64::from(week)
            .checked_mul(7)
            .and_then(|days| days.checked_add(i64::from(day_of_week)))
            .ok_or(DataCatalogError::DateOutOfRange)?;
        product_date_from_jdn(
            epoch_jdn
                .checked_add(offset_days)
                .ok_or(DataCatalogError::DateOutOfRange)?,
        )
    }

    /// GPS week for this date.
    pub fn gps_week(self) -> Result<u32, DataCatalogError> {
        week_from_calendar(
            TimeScale::Gpst,
            i64::from(self.year),
            i64::from(self.month),
            i64::from(self.day),
        )
        .ok_or(DataCatalogError::DateBeforeGpsEpoch(self))
    }

    /// Day-of-year in `1..=366`.
    #[must_use]
    pub fn day_of_year(self) -> u16 {
        day_of_year_int(self.year, i32::from(self.month), i32::from(self.day)) as u16
    }

    fn add_days(self, days: i64) -> Result<Self, DataCatalogError> {
        product_date_from_jdn(
            self.julian_day_number()
                .checked_add(days)
                .ok_or(DataCatalogError::DateOutOfRange)?,
        )
    }

    fn julian_day_number(self) -> i64 {
        julian_day_number(self.year, i32::from(self.month), i32::from(self.day))
    }
}

impl fmt::Display for ProductDate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04}-{:02}-{:02}", self.year, self.month, self.day)
    }
}

/// Civil UTC date and time used for ultra-rapid issue selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ProductDateTime {
    /// Date.
    pub date: ProductDate,
    /// Hour in `0..=23`.
    pub hour: u8,
    /// Minute in `0..=59`.
    pub minute: u8,
    /// Second in `0..=59`.
    pub second: u8,
}

impl ProductDateTime {
    /// Build and validate a civil date and time.
    pub fn new(
        date: ProductDate,
        hour: u8,
        minute: u8,
        second: u8,
    ) -> Result<Self, DataCatalogError> {
        if hour > 23 || minute > 59 || second > 59 {
            return Err(DataCatalogError::InvalidDateTime {
                hour,
                minute,
                second,
            });
        }
        Ok(Self {
            date,
            hour,
            minute,
            second,
        })
    }

    fn ordering_minutes(self) -> i64 {
        self.date.julian_day_number() * 1_440 + i64::from(self.hour) * 60 + i64::from(self.minute)
    }
}

/// Ultra-rapid issue date and `HHMM` issue time.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct UltraIssue {
    /// Product date.
    pub date: ProductDate,
    /// Issue time.
    pub issue: String,
}

impl UltraIssue {
    /// Build and validate an ultra-rapid issue.
    pub fn new(date: ProductDate, issue: &str) -> Result<Self, DataCatalogError> {
        validate_issue(issue)?;
        Ok(Self {
            date,
            issue: issue.to_string(),
        })
    }
}

/// A pure product specification that resolves to one archive filename and URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductSpec {
    /// Analysis center.
    pub center: AnalysisCenter,
    /// Product type.
    pub product_type: ProductType,
    /// Product date.
    pub date: ProductDate,
    /// Sampling token.
    pub sample: String,
    /// Optional issue time for ultra-rapid products.
    pub issue: Option<String>,
}

impl ProductSpec {
    /// Build a product specification and validate it against the catalog.
    pub fn new(
        center: AnalysisCenter,
        product_type: ProductType,
        date: ProductDate,
        sample: &str,
        issue: Option<&str>,
    ) -> Result<Self, DataCatalogError> {
        validate_product(center, product_type, sample, issue)?;
        Ok(Self {
            center,
            product_type,
            date,
            sample: sample.to_string(),
            issue: issue.map(ToOwned::to_owned),
        })
    }

    /// GPS week for the product date.
    pub fn gps_week(&self) -> Result<u32, DataCatalogError> {
        self.date.gps_week()
    }

    /// Day-of-year for the product date.
    #[must_use]
    pub fn day_of_year(&self) -> u16 {
        self.date.day_of_year()
    }

    /// Canonical IGS long-name filename without archive compression suffix.
    pub fn canonical_filename(&self) -> Result<String, DataCatalogError> {
        let convention = validate_product(
            self.center,
            self.product_type,
            &self.sample,
            self.issue.as_deref(),
        )?;
        let descriptor = product_type_convention(self.product_type);
        Ok(match descriptor.kind {
            ProductFilenameKind::Sampled => format!(
                "{}_{}_{}_{}_{}.{}",
                convention.token,
                date_block(self.date, self.issue.as_deref()),
                convention.span,
                self.sample,
                descriptor.content_code,
                descriptor.extension
            ),
            ProductFilenameKind::Nav => format!(
                "{}_R_{}_{}_{}.{}",
                convention.token,
                date_block(self.date, None),
                convention.span,
                descriptor.content_code,
                descriptor.extension
            ),
        })
    }

    /// Full archive URL, including `.gz` when the cataloged archive is gzipped.
    pub fn archive_url(&self) -> Result<String, DataCatalogError> {
        let convention = validate_product(
            self.center,
            self.product_type,
            &self.sample,
            self.issue.as_deref(),
        )?;
        let entry = center_catalog(self.center).expect("catalog entry exists for enum variant");
        let filename = self.canonical_filename()?;
        Ok(format!(
            "{}/{}/{}{}",
            entry.root_url,
            dir_path(convention.layout, self.date)?,
            filename,
            convention.compression.suffix()
        ))
    }
}

/// A pure station observation specification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StationObservationSpec {
    /// 9-character RINEX 3 site identifier.
    pub station: String,
    /// Observation date.
    pub date: ProductDate,
    /// Sampling token.
    pub sample: String,
}

impl StationObservationSpec {
    /// Build and validate a daily station observation product.
    pub fn new(station: &str, date: ProductDate, sample: &str) -> Result<Self, DataCatalogError> {
        validate_station(station)?;
        validate_sample(sample)?;
        Ok(Self {
            station: station.to_string(),
            date,
            sample: sample.to_string(),
        })
    }

    /// Canonical RINEX 3 CRINEX filename without archive compression suffix.
    pub fn canonical_filename(&self) -> Result<String, DataCatalogError> {
        station_obs_filename(&self.station, self.date, &self.sample)
    }

    /// Full archive URL, including `.gz`.
    pub fn archive_url(&self) -> Result<String, DataCatalogError> {
        station_obs_url(&self.station, self.date, &self.sample)
    }
}

/// Static catalog entries, in the same order as the binding data catalog.
#[must_use]
pub const fn catalog() -> &'static [CenterCatalogEntry] {
    &CATALOG
}

/// Supported center codes, in catalog order.
#[must_use]
pub const fn centers() -> &'static [AnalysisCenter] {
    &CENTER_ORDER
}

/// Supported product types.
#[must_use]
pub const fn product_types() -> &'static [ProductTypeConvention] {
    &PRODUCT_TYPE_CONVENTIONS
}

/// Archive hosts present in the catalog.
#[must_use]
pub const fn allowed_hosts() -> &'static [&'static str] {
    &ALLOWED_HOSTS
}

/// Product pairs intentionally withheld because no open mirror is known.
#[must_use]
pub const fn no_open_mirrors() -> &'static [NoOpenMirrorProduct] {
    &NO_OPEN_MIRRORS
}

/// Confirm that a center/product pair has an open catalog mirror.
pub fn open_mirror(
    center: AnalysisCenter,
    product_type: ProductType,
) -> Result<(), DataCatalogError> {
    open_mirror_code(center.code(), product_type.code())
}

/// Confirm that a center/product code pair is not in the no-open-mirror list.
pub fn open_mirror_code(center: &str, product_type: &str) -> Result<(), DataCatalogError> {
    if NO_OPEN_MIRRORS
        .iter()
        .any(|entry| entry.center == center && entry.product_type == product_type)
    {
        Err(DataCatalogError::NoOpenMirror {
            center: center.to_string(),
            product_type: product_type.to_string(),
        })
    } else {
        Ok(())
    }
}

/// Look up a center's static catalog entry.
#[must_use]
pub fn center_catalog(center: AnalysisCenter) -> Option<&'static CenterCatalogEntry> {
    CATALOG.iter().find(|entry| entry.center == center)
}

/// Look up the convention for one center and product type.
pub fn product_convention(
    center: AnalysisCenter,
    product_type: ProductType,
) -> Result<&'static CenterProductConvention, DataCatalogError> {
    open_mirror(center, product_type)?;
    let entry = center_catalog(center).expect("catalog entry exists for enum variant");
    entry
        .products
        .iter()
        .find(|product| product.product_type == product_type)
        .ok_or(DataCatalogError::UnsupportedProduct {
            center,
            product_type,
        })
}

/// Default sampling token for a center/product pair.
pub fn default_sample(
    center: AnalysisCenter,
    product_type: ProductType,
) -> Result<&'static str, DataCatalogError> {
    Ok(product_convention(center, product_type)?.default_sample)
}

/// GPS week number for a product date.
pub fn gps_week(date: ProductDate) -> Result<u32, DataCatalogError> {
    date.gps_week()
}

/// Day-of-year in `1..=366` for a product date.
#[must_use]
pub fn day_of_year(date: ProductDate) -> u16 {
    date.day_of_year()
}

/// Build a product specification for any center/product/date combination.
pub fn product(
    center: AnalysisCenter,
    product_type: ProductType,
    date: ProductDate,
    sample: Option<&str>,
    issue: Option<&str>,
) -> Result<ProductSpec, DataCatalogError> {
    let sample = match sample {
        Some(sample) => sample,
        None => default_sample(center, product_type)?,
    };
    ProductSpec::new(center, product_type, date, sample, issue)
}

/// Build the canonical IGS long-name filename for a product.
pub fn canonical_filename(
    center: AnalysisCenter,
    product_type: ProductType,
    date: ProductDate,
    sample: Option<&str>,
    issue: Option<&str>,
) -> Result<String, DataCatalogError> {
    product(center, product_type, date, sample, issue)?.canonical_filename()
}

/// Build the full archive URL for a product.
pub fn archive_url(
    center: AnalysisCenter,
    product_type: ProductType,
    date: ProductDate,
    sample: Option<&str>,
    issue: Option<&str>,
) -> Result<String, DataCatalogError> {
    product(center, product_type, date, sample, issue)?.archive_url()
}

/// Build a clock product for a center and date.
pub fn mgex_clk(
    center: AnalysisCenter,
    date: ProductDate,
    sample: Option<&str>,
) -> Result<ProductSpec, DataCatalogError> {
    product(center, ProductType::Clk, date, sample, None)
}

/// Build a merged broadcast-navigation product for a center and date.
pub fn mgex_nav(
    center: AnalysisCenter,
    date: ProductDate,
    sample: Option<&str>,
) -> Result<ProductSpec, DataCatalogError> {
    product(center, ProductType::Nav, date, sample, None)
}

/// Build an IONEX product for a center and date.
pub fn mgex_ionex(
    center: AnalysisCenter,
    date: ProductDate,
    sample: Option<&str>,
) -> Result<ProductSpec, DataCatalogError> {
    product(center, ProductType::Ionex, date, sample, None)
}

/// Build the CODE rapid IONEX product for a date.
pub fn rapid_ionex(
    date: ProductDate,
    sample: Option<&str>,
) -> Result<ProductSpec, DataCatalogError> {
    product(
        AnalysisCenter::CodRap,
        ProductType::Ionex,
        date,
        sample,
        None,
    )
}

/// Day offset for predicted IONEX aliases.
#[must_use]
pub const fn predicted_day_offset(center: AnalysisCenter) -> i64 {
    match center {
        AnalysisCenter::CodPrd2 => 1,
        _ => 0,
    }
}

/// Build a CODE predicted IONEX product for a target date.
pub fn predicted_ionex(
    center: AnalysisCenter,
    date: ProductDate,
    sample: Option<&str>,
) -> Result<ProductSpec, DataCatalogError> {
    match center {
        AnalysisCenter::CodPrd1 | AnalysisCenter::CodPrd2 => {
            let target = date.add_days(predicted_day_offset(center))?;
            product(center, ProductType::Ionex, target, sample, None)
        }
        other => Err(DataCatalogError::UnsupportedProduct {
            center: other,
            product_type: ProductType::Ionex,
        }),
    }
}

/// Build an SP3 product for a center and date.
pub fn mgex_sp3(
    center: AnalysisCenter,
    date: ProductDate,
    sample: Option<&str>,
) -> Result<ProductSpec, DataCatalogError> {
    product(center, ProductType::Sp3, date, sample, None)
}

/// Build an ultra-rapid OPS SP3 product for a date and issue time.
pub fn ops_ultra_sp3(
    center: AnalysisCenter,
    date: ProductDate,
    sample: Option<&str>,
    issue: Option<&str>,
) -> Result<ProductSpec, DataCatalogError> {
    let issue = issue.unwrap_or("0000");
    product(center, ProductType::Sp3, date, sample, Some(issue))
}

/// Build an ultra-rapid OPS clock product for a date and issue time.
pub fn ops_ultra_clk(
    center: AnalysisCenter,
    date: ProductDate,
    sample: Option<&str>,
    issue: Option<&str>,
) -> Result<ProductSpec, DataCatalogError> {
    let issue = issue.unwrap_or("0000");
    product(center, ProductType::Clk, date, sample, Some(issue))
}

/// Select the latest ultra-rapid OPS SP3 issue at or before a target time.
pub fn latest_ops_ultra_sp3(
    center: AnalysisCenter,
    target: ProductDateTime,
    sample: Option<&str>,
    available_issues: Option<&[UltraIssue]>,
) -> Result<ProductSpec, DataCatalogError> {
    let selected = latest_ultra_issue(center, target, available_issues)?;
    ops_ultra_sp3(center, selected.date, sample, Some(&selected.issue))
}

/// Candidate ultra-rapid issues at or before a target time, newest first.
pub fn ultra_issue_candidates(
    center: AnalysisCenter,
    target: ProductDateTime,
) -> Result<Vec<UltraIssue>, DataCatalogError> {
    let entry = center_catalog(center).expect("catalog entry exists for enum variant");
    let _ = product_convention(center, ProductType::Sp3)?;
    if entry.issues.is_empty() {
        return Err(DataCatalogError::UnsupportedProduct {
            center,
            product_type: ProductType::Sp3,
        });
    }

    let mut candidates = Vec::new();
    for date in [target.date, target.date.add_days(-1)?] {
        for issue in entry.issues.iter().rev() {
            if issue_ordering_minutes(date, issue)? <= target.ordering_minutes() {
                candidates.push(UltraIssue::new(date, issue)?);
            }
        }
    }
    Ok(candidates)
}

/// Latest ultra-rapid issue at or before a target time.
pub fn latest_ultra_issue(
    center: AnalysisCenter,
    target: ProductDateTime,
    available_issues: Option<&[UltraIssue]>,
) -> Result<UltraIssue, DataCatalogError> {
    let candidates = ultra_issue_candidates(center, target)?;
    if candidates.is_empty() {
        return Err(DataCatalogError::NoUltraIssue);
    }
    if let Some(available) = available_issues {
        candidates
            .into_iter()
            .find(|candidate| {
                available
                    .iter()
                    .any(|issue| issue.date == candidate.date && issue.issue == candidate.issue)
            })
            .ok_or(DataCatalogError::NoAvailableUltraIssue)
    } else {
        Ok(candidates[0].clone())
    }
}

/// Candidate IONEX dates at or before a target date, newest first.
pub fn gim_date_candidates(
    center: AnalysisCenter,
    target: ProductDate,
    lookback: u32,
) -> Result<Vec<ProductDate>, DataCatalogError> {
    let _ = product_convention(center, ProductType::Ionex)?;
    let base = target.add_days(predicted_day_offset(center))?;
    let mut out = Vec::with_capacity(usize::try_from(lookback).unwrap_or(usize::MAX));
    for back in 0..=lookback {
        out.push(base.add_days(-i64::from(back))?);
    }
    Ok(out)
}

/// Build a daily station observation product.
pub fn station_obs(
    station: &str,
    date: ProductDate,
    sample: Option<&str>,
) -> Result<StationObservationSpec, DataCatalogError> {
    StationObservationSpec::new(station, date, sample.unwrap_or("30S"))
}

/// Build the canonical RINEX 3 CRINEX filename for a daily station observation.
pub fn station_obs_filename(
    station: &str,
    date: ProductDate,
    sample: &str,
) -> Result<String, DataCatalogError> {
    validate_station(station)?;
    validate_sample(sample)?;
    Ok(format!(
        "{}_R_{}_01D_{}_MO.crx",
        station,
        date_block(date, None),
        sample
    ))
}

/// Build the full BKG IGS archive URL for a daily station observation.
pub fn station_obs_url(
    station: &str,
    date: ProductDate,
    sample: &str,
) -> Result<String, DataCatalogError> {
    let filename = station_obs_filename(station, date, sample)?;
    Ok(format!(
        "https://igs.bkg.bund.de/root_ftp/IGS/{}/{}.gz",
        dir_path(ArchiveLayout::BkgObsYearDoy, date)?,
        filename
    ))
}

/// The transfer protocol for the daily station observation archive.
#[must_use]
pub const fn station_obs_protocol() -> ArchiveProtocol {
    ArchiveProtocol::Https
}

fn product_type_convention(product_type: ProductType) -> &'static ProductTypeConvention {
    PRODUCT_TYPE_CONVENTIONS
        .iter()
        .find(|descriptor| descriptor.product_type == product_type)
        .expect("product descriptor exists for enum variant")
}

fn validate_product(
    center: AnalysisCenter,
    product_type: ProductType,
    sample: &str,
    issue: Option<&str>,
) -> Result<&'static CenterProductConvention, DataCatalogError> {
    let convention = product_convention(center, product_type)?;
    validate_sample(sample)?;
    validate_issue_for_center(center, issue)?;
    Ok(convention)
}

fn validate_issue_for_center(
    center: AnalysisCenter,
    issue: Option<&str>,
) -> Result<(), DataCatalogError> {
    let entry = center_catalog(center).expect("catalog entry exists for enum variant");
    match (entry.issues.is_empty(), issue) {
        (true, None) => Ok(()),
        (true, Some(_)) => Err(DataCatalogError::UnexpectedIssue { center }),
        (false, None) => Err(DataCatalogError::MissingIssue { center }),
        (false, Some(issue)) => {
            validate_issue(issue)?;
            if entry.issues.contains(&issue) {
                Ok(())
            } else {
                Err(DataCatalogError::UnsupportedIssue {
                    center,
                    issue: issue.to_string(),
                })
            }
        }
    }
}

fn validate_sample(sample: &str) -> Result<(), DataCatalogError> {
    let bytes = sample.as_bytes();
    let valid = bytes.len() == 3
        && bytes[0].is_ascii_digit()
        && bytes[1].is_ascii_digit()
        && bytes[2].is_ascii_uppercase();
    if valid {
        Ok(())
    } else {
        Err(DataCatalogError::InvalidSample(sample.to_string()))
    }
}

fn validate_issue(issue: &str) -> Result<(), DataCatalogError> {
    let bytes = issue.as_bytes();
    let valid_digits = bytes.len() == 4 && bytes.iter().all(u8::is_ascii_digit);
    if !valid_digits {
        return Err(DataCatalogError::InvalidIssue(issue.to_string()));
    }
    let hour = issue[0..2]
        .parse::<u8>()
        .map_err(|_| DataCatalogError::InvalidIssue(issue.to_string()))?;
    let minute = issue[2..4]
        .parse::<u8>()
        .map_err(|_| DataCatalogError::InvalidIssue(issue.to_string()))?;
    if hour <= 23 && minute <= 59 {
        Ok(())
    } else {
        Err(DataCatalogError::InvalidIssue(issue.to_string()))
    }
}

fn validate_station(station: &str) -> Result<(), DataCatalogError> {
    let bytes = station.as_bytes();
    let valid = bytes.len() == 9
        && bytes
            .iter()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit());
    if valid {
        Ok(())
    } else {
        Err(DataCatalogError::InvalidStation(station.to_string()))
    }
}

fn issue_minutes(issue: &str) -> Result<u16, DataCatalogError> {
    validate_issue(issue)?;
    let hour = issue[0..2]
        .parse::<u16>()
        .map_err(|_| DataCatalogError::InvalidIssue(issue.to_string()))?;
    let minute = issue[2..4]
        .parse::<u16>()
        .map_err(|_| DataCatalogError::InvalidIssue(issue.to_string()))?;
    Ok(hour * 60 + minute)
}

fn issue_ordering_minutes(date: ProductDate, issue: &str) -> Result<i64, DataCatalogError> {
    Ok(date.julian_day_number() * 1_440 + i64::from(issue_minutes(issue)?))
}

fn date_block(date: ProductDate, issue: Option<&str>) -> String {
    format!(
        "{}{:03}{}",
        date.year,
        date.day_of_year(),
        issue.unwrap_or("0000")
    )
}

fn dir_path(layout: ArchiveLayout, date: ProductDate) -> Result<String, DataCatalogError> {
    Ok(match layout {
        ArchiveLayout::GfzRapidWeek => format!("rapid/w{}", date.gps_week()?),
        ArchiveLayout::GfzUltraWeek => format!("ultra/w{}", date.gps_week()?),
        ArchiveLayout::GpsWeek => date.gps_week()?.to_string(),
        ArchiveLayout::BkgProductsWeek => format!("products/{}", date.gps_week()?),
        ArchiveLayout::BkgBrdcYearDoy => {
            format!("BRDC/{}/{:03}", date.year, date.day_of_year())
        }
        ArchiveLayout::BkgObsYearDoy => format!("obs/{}/{:03}", date.year, date.day_of_year()),
        ArchiveLayout::AiubCodeMgexYear => format!("CODE_MGEX/CODE/{}", date.year),
        ArchiveLayout::AiubCodeYear => format!("CODE/{}", date.year),
        ArchiveLayout::AiubCodeRoot => "CODE".to_string(),
    })
}

fn product_date_from_jdn(jdn: i64) -> Result<ProductDate, DataCatalogError> {
    let (year, month, day) = civil_from_julian_day_number(jdn);
    let year = i32::try_from(year).map_err(|_| DataCatalogError::DateOutOfRange)?;
    let month = u8::try_from(month).map_err(|_| DataCatalogError::DateOutOfRange)?;
    let day = u8::try_from(day).map_err(|_| DataCatalogError::DateOutOfRange)?;
    ProductDate::new(year, month, day).map_err(|_| DataCatalogError::DateOutOfRange)
}
