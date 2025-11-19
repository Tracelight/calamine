// SPDX-License-Identifier: MIT
//
// Copyright 2016-2025, Johann Tuffe.

use std::collections::HashMap;

/// Legacy comment (pre-Office 365) on a cell
#[derive(Debug, Clone, PartialEq)]
pub struct Comment {
    pub cell_ref: String,
    /// Index into the authors array for this worksheet
    pub author_id: usize,
    pub text: Vec<RichTextRun>,
    pub position: Option<Position>,
}

/// Threaded comment (Office 365+) on a cell
#[derive(Debug, Clone, PartialEq)]
pub struct ThreadedComment {
    /// Unique identifier (GUID)
    pub id: String,
    pub cell_ref: String,
    /// Reference to Person via persons() - GUID
    pub person_id: String,
    pub text: String,
    /// If this is a reply, the parent comment's GUID
    pub parent_id: Option<String>,
    pub timestamp: Option<String>,
}

/// Person metadata for threaded comments
#[derive(Debug, Clone, PartialEq)]
pub struct Person {
    /// Unique identifier (GUID)
    pub id: String,
    pub display_name: String,
    /// Email or username
    pub user_id: Option<String>,
}

/// A formatted text segment in a legacy comment
#[derive(Debug, Clone, PartialEq)]
pub struct RichTextRun {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    /// RGB hex color (e.g., "FF0000" for red)
    pub color: Option<String>,
}

/// Visual position and size of a legacy comment
#[derive(Debug, Clone, PartialEq)]
pub struct Position {
    pub row: f64,
    pub column: f64,
    pub width: f64,
    pub height: f64,
}

/// Collection of legacy comments for a worksheet
pub type LegacyComments = (Vec<String>, Vec<Comment>);

/// Map of sheet name to legacy comments (authors, comments)
pub type LegacyCommentsMap = HashMap<String, LegacyComments>;

/// Map of sheet name to threaded comments
pub type ThreadedCommentsMap = HashMap<String, Vec<ThreadedComment>>;

/// Map of person ID to person metadata
pub type PersonsMap = HashMap<String, Person>;
