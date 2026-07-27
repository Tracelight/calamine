// SPDX-License-Identifier: MIT
//
// Copyright 2016-2025, Johann Tuffe.

//! Parsed conditional-formatting rules from worksheet
//! `<conditionalFormatting>` blocks.
//!
//! The reader applies schema defaults, resolves colours through the workbook
//! palette, and preserves unknown enum values in `Other`.

use std::fmt;

use crate::{style::Color, Dimensions};

/// The `type` attribute of a `<cfRule>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConditionalFormatRuleType {
    /// The required `type` attribute was absent.
    Missing,
    /// An arbitrary formula evaluated per cell.
    Expression,
    /// A comparison of the cell value against one or two operands.
    CellIs,
    /// Continuous interpolation between colour stops.
    ColorScale,
    /// In-cell bars scaled between two bounds.
    DataBar,
    /// Icons bucketed by threshold.
    IconSet,
    /// Top/bottom N items or percent.
    Top10,
    /// Values occurring exactly once in the applied range.
    UniqueValues,
    /// Values occurring more than once in the applied range.
    DuplicateValues,
    /// Substring match.
    ContainsText,
    /// Negated substring match.
    NotContainsText,
    /// Prefix match.
    BeginsWith,
    /// Suffix match.
    EndsWith,
    /// Empty cells.
    ContainsBlanks,
    /// Non-empty cells.
    NotContainsBlanks,
    /// Cells holding an error value.
    ContainsErrors,
    /// Cells not holding an error value.
    NotContainsErrors,
    /// Dates falling in a named period.
    TimePeriod,
    /// Values above or below the range average.
    AboveAverage,
    /// A rule type this reader does not model, kept verbatim.
    Other(String),
}

impl ConditionalFormatRuleType {
    /// Parses the `type` attribute, falling back to [`Self::Other`].
    pub fn from_attribute(value: &str) -> Self {
        match value {
            "expression" => Self::Expression,
            "cellIs" => Self::CellIs,
            "colorScale" => Self::ColorScale,
            "dataBar" => Self::DataBar,
            "iconSet" => Self::IconSet,
            "top10" => Self::Top10,
            "uniqueValues" => Self::UniqueValues,
            "duplicateValues" => Self::DuplicateValues,
            "containsText" => Self::ContainsText,
            "notContainsText" => Self::NotContainsText,
            "beginsWith" => Self::BeginsWith,
            "endsWith" => Self::EndsWith,
            "containsBlanks" => Self::ContainsBlanks,
            "notContainsBlanks" => Self::NotContainsBlanks,
            "containsErrors" => Self::ContainsErrors,
            "notContainsErrors" => Self::NotContainsErrors,
            "timePeriod" => Self::TimePeriod,
            "aboveAverage" => Self::AboveAverage,
            other => Self::Other(other.to_string()),
        }
    }
}

impl fmt::Display for ConditionalFormatRuleType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            Self::Missing => "",
            Self::Expression => "expression",
            Self::CellIs => "cellIs",
            Self::ColorScale => "colorScale",
            Self::DataBar => "dataBar",
            Self::IconSet => "iconSet",
            Self::Top10 => "top10",
            Self::UniqueValues => "uniqueValues",
            Self::DuplicateValues => "duplicateValues",
            Self::ContainsText => "containsText",
            Self::NotContainsText => "notContainsText",
            Self::BeginsWith => "beginsWith",
            Self::EndsWith => "endsWith",
            Self::ContainsBlanks => "containsBlanks",
            Self::NotContainsBlanks => "notContainsBlanks",
            Self::ContainsErrors => "containsErrors",
            Self::NotContainsErrors => "notContainsErrors",
            Self::TimePeriod => "timePeriod",
            Self::AboveAverage => "aboveAverage",
            Self::Other(raw) => raw,
        };
        f.write_str(s)
    }
}

/// The `operator` attribute of a `<cfRule>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConditionalFormatOperator {
    /// `lessThan`
    LessThan,
    /// `lessThanOrEqual`
    LessThanOrEqual,
    /// `equal`
    Equal,
    /// `notEqual`
    NotEqual,
    /// `greaterThanOrEqual`
    GreaterThanOrEqual,
    /// `greaterThan`
    GreaterThan,
    /// `between`, using both formulas
    Between,
    /// `notBetween`, using both formulas
    NotBetween,
    /// `containsText`
    ContainsText,
    /// `notContains`
    NotContains,
    /// `beginsWith`
    BeginsWith,
    /// `endsWith`
    EndsWith,
    /// An operator this reader does not model, kept verbatim.
    Other(String),
}

impl ConditionalFormatOperator {
    /// Parses the `operator` attribute, falling back to [`Self::Other`].
    pub fn from_attribute(value: &str) -> Self {
        match value {
            "lessThan" => Self::LessThan,
            "lessThanOrEqual" => Self::LessThanOrEqual,
            "equal" => Self::Equal,
            "notEqual" => Self::NotEqual,
            "greaterThanOrEqual" => Self::GreaterThanOrEqual,
            "greaterThan" => Self::GreaterThan,
            "between" => Self::Between,
            "notBetween" => Self::NotBetween,
            "containsText" => Self::ContainsText,
            "notContains" => Self::NotContains,
            "beginsWith" => Self::BeginsWith,
            "endsWith" => Self::EndsWith,
            other => Self::Other(other.to_string()),
        }
    }
}

/// The `timePeriod` attribute of a `<cfRule>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConditionalFormatTimePeriod {
    /// `today`
    Today,
    /// `yesterday`
    Yesterday,
    /// `tomorrow`
    Tomorrow,
    /// `last7Days`
    Last7Days,
    /// `thisWeek`
    ThisWeek,
    /// `lastWeek`
    LastWeek,
    /// `nextWeek`
    NextWeek,
    /// `thisMonth`
    ThisMonth,
    /// `lastMonth`
    LastMonth,
    /// `nextMonth`
    NextMonth,
    /// A period this reader does not model, kept verbatim.
    Other(String),
}

impl ConditionalFormatTimePeriod {
    /// Parses the `timePeriod` attribute, falling back to [`Self::Other`].
    pub fn from_attribute(value: &str) -> Self {
        match value {
            "today" => Self::Today,
            "yesterday" => Self::Yesterday,
            "tomorrow" => Self::Tomorrow,
            "last7Days" => Self::Last7Days,
            "thisWeek" => Self::ThisWeek,
            "lastWeek" => Self::LastWeek,
            "nextWeek" => Self::NextWeek,
            "thisMonth" => Self::ThisMonth,
            "lastMonth" => Self::LastMonth,
            "nextMonth" => Self::NextMonth,
            other => Self::Other(other.to_string()),
        }
    }
}

/// How a `<cfvo>` threshold is derived.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConditionalFormatValueKind {
    /// The required `type` attribute was absent.
    Missing,
    /// The range minimum.
    Min,
    /// The range maximum.
    Max,
    /// The smaller of zero and the range minimum.
    AutoMin,
    /// The larger of zero and the range maximum.
    AutoMax,
    /// A literal number in `val`.
    Number,
    /// A percentage of the range span.
    Percent,
    /// A percentile of the range values.
    Percentile,
    /// A formula in `val`.
    Formula,
    /// A kind this reader does not model, kept verbatim.
    Other(String),
}

impl ConditionalFormatValueKind {
    /// Parses the `type` attribute of a `<cfvo>`, falling back to [`Self::Other`].
    pub fn from_attribute(value: &str) -> Self {
        match value {
            "min" => Self::Min,
            "max" => Self::Max,
            "autoMin" => Self::AutoMin,
            "autoMax" => Self::AutoMax,
            "num" => Self::Number,
            "percent" => Self::Percent,
            "percentile" => Self::Percentile,
            "formula" => Self::Formula,
            other => Self::Other(other.to_string()),
        }
    }
}

/// A `<cfvo>` threshold on a colour scale, data bar or icon set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConditionalFormatValue {
    /// How the threshold is derived.
    pub kind: ConditionalFormatValueKind,
    /// The `val` attribute, a literal or a formula depending on `kind`.
    pub value: Option<String>,
    /// Whether the icon-set bucket boundary is inclusive.
    ///
    /// An absent `gte` defaults to true.
    pub inclusive: bool,
}

/// One parsed `<cfRule>`.
///
/// Fields irrelevant to `rule_type` retain their OOXML defaults.
#[derive(Debug, Clone, PartialEq)]
pub struct ConditionalFormatRule {
    /// The rule's `type`.
    pub rule_type: ConditionalFormatRuleType,
    /// Evaluation order within the worksheet; lower wins.
    ///
    /// `None` when the required `priority` attribute was absent or unparsable.
    pub priority: Option<i32>,
    /// Whether a match suppresses all lower-priority rules for that cell.
    pub stop_if_true: bool,
    /// Index into the workbook's `dxfs` differential formats.
    pub dxf_id: Option<u32>,
    /// `<formula>` children, in document order, written relative to the
    /// top-left cell of the applied range.
    pub formulas: Vec<String>,
    /// Comparison operator, for `cellIs` and the text rules.
    pub operator: Option<ConditionalFormatOperator>,
    /// Search term, for the text rules.
    pub text: Option<String>,
    /// Named date period, for `timePeriod`.
    pub time_period: Option<ConditionalFormatTimePeriod>,
    /// Item or percent count, for `top10`.
    pub rank: Option<u32>,
    /// Whether `rank` is a percent rather than a count.
    pub rank_percent: bool,
    /// Whether `top10` selects from the bottom.
    pub bottom: bool,
    /// Whether `aboveAverage` selects above rather than below. Defaults to true.
    pub above_average: bool,
    /// Whether `aboveAverage` includes values equal to the average.
    pub equal_average: bool,
    /// Standard-deviation band, for `aboveAverage`.
    pub std_dev: Option<i32>,
    /// Thresholds for the scale families, in document order.
    pub values: Vec<ConditionalFormatValue>,
    /// Colours for the scale families, in document order.
    ///
    /// An unresolvable colour remains as `None`, preserving its position without
    /// inventing a colour that was not present in the file.
    pub colors: Vec<Option<Color>>,
    /// Shortest data bar length, as a percentage of cell width.
    pub min_length: u32,
    /// Longest data bar length, as a percentage of cell width.
    pub max_length: u32,
    /// The `iconSet` attribute naming the icon family, e.g. `3Arrows`.
    pub icon_set: Option<String>,
    /// Whether icon-set thresholds are percentages rather than numeric values.
    pub icon_percent: bool,
    /// Whether the icon order is reversed.
    pub reverse_icons: bool,
    /// Whether the cell value is shown alongside the bar or icon. Defaults to true.
    pub show_value: bool,
}

impl ConditionalFormatRule {
    /// Creates a rule with OOXML defaults for every field except `rule_type`.
    pub fn new(rule_type: ConditionalFormatRuleType) -> Self {
        Self {
            rule_type,
            priority: None,
            stop_if_true: false,
            dxf_id: None,
            formulas: Vec::new(),
            operator: None,
            text: None,
            time_period: None,
            rank: None,
            rank_percent: false,
            bottom: false,
            above_average: true,
            equal_average: false,
            std_dev: None,
            values: Vec::new(),
            colors: Vec::new(),
            min_length: 10,
            max_length: 90,
            icon_set: None,
            icon_percent: true,
            reverse_icons: false,
            show_value: true,
        }
    }
}

/// A `<conditionalFormatting>` block: rules sharing an applied range.
///
/// Rules in separate blocks can interleave because `priority` is
/// worksheet-wide.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ConditionalFormatting {
    /// Whether the block applies to a PivotTable.
    pub pivot: bool,
    /// The `sqref` areas, in document order.
    pub ranges: Vec<Dimensions>,
    /// The block's rules, in document order.
    pub rules: Vec<ConditionalFormatRule>,
}
