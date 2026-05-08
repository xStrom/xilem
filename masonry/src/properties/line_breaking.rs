// Copyright 2025 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use crate::core::Property;

/// Options for handling lines of text that are too wide for the available space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LineBreaking {
    /// Lines are broken at word boundaries.
    WordWrap,
    /// Lines are truncated to the width of the label.
    Clip,
    /// Lines overflow the label.
    Overflow,
}

impl Property for LineBreaking {
    fn static_default() -> &'static Self {
        &Self::Overflow
    }
}

impl Default for LineBreaking {
    fn default() -> Self {
        *Self::static_default()
    }
}
