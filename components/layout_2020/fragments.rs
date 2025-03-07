/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use crate::geom::flow_relative::{Rect, Sides};
use crate::style_ext::{Direction, WritingMode};
use gfx::text::glyph::GlyphStore;
use servo_arc::Arc as ServoArc;
use std::sync::Arc;
use style::properties::ComputedValues;
use style::values::computed::Length;
use style::Zero;
use webrender_api::{FontInstanceKey, ImageKey};

pub(crate) enum Fragment {
    Box(BoxFragment),
    Anonymous(AnonymousFragment),
    Text(TextFragment),
    Image(ImageFragment),
}

pub(crate) struct BoxFragment {
    pub style: ServoArc<ComputedValues>,
    pub children: Vec<Fragment>,

    /// From the containing block’s start corner…?
    /// This might be broken when the containing block is in a different writing mode:
    /// https://drafts.csswg.org/css-writing-modes/#orthogonal-flows
    pub content_rect: Rect<Length>,

    pub padding: Sides<Length>,
    pub border: Sides<Length>,
    pub margin: Sides<Length>,

    pub block_margins_collapsed_with_children: CollapsedBlockMargins,
}

pub(crate) struct CollapsedBlockMargins {
    pub collapsed_through: bool,
    pub start: CollapsedMargin,
    pub end: CollapsedMargin,
}

#[derive(Clone, Copy)]
pub(crate) struct CollapsedMargin {
    max_positive: Length,
    min_negative: Length,
}

/// Can contain child fragments with relative coordinates, but does not contribute to painting itself.
pub(crate) struct AnonymousFragment {
    pub rect: Rect<Length>,
    pub children: Vec<Fragment>,
    pub mode: (WritingMode, Direction),
}

pub(crate) struct TextFragment {
    pub parent_style: ServoArc<ComputedValues>,
    pub content_rect: Rect<Length>,
    pub ascent: Length,
    pub font_key: FontInstanceKey,
    pub glyphs: Vec<Arc<GlyphStore>>,
}

pub(crate) struct ImageFragment {
    pub style: ServoArc<ComputedValues>,
    pub content_rect: Rect<Length>,
    pub image_key: ImageKey,
}

impl AnonymousFragment {
    pub fn no_op(mode: (WritingMode, Direction)) -> Self {
        Self {
            children: vec![],
            rect: Rect::zero(),
            mode,
        }
    }
}

impl BoxFragment {
    pub fn border_rect(&self) -> Rect<Length> {
        self.content_rect
            .inflate(&self.padding)
            .inflate(&self.border)
    }
}

impl CollapsedBlockMargins {
    pub fn from_margin(margin: &Sides<Length>) -> Self {
        Self {
            collapsed_through: false,
            start: CollapsedMargin::new(margin.block_start),
            end: CollapsedMargin::new(margin.block_end),
        }
    }

    pub fn zero() -> Self {
        Self {
            collapsed_through: false,
            start: CollapsedMargin::zero(),
            end: CollapsedMargin::zero(),
        }
    }
}

impl CollapsedMargin {
    pub fn zero() -> Self {
        Self {
            max_positive: Length::zero(),
            min_negative: Length::zero(),
        }
    }

    pub fn new(margin: Length) -> Self {
        Self {
            max_positive: margin.max(Length::zero()),
            min_negative: margin.min(Length::zero()),
        }
    }

    pub fn adjoin(&self, other: &Self) -> Self {
        Self {
            max_positive: self.max_positive.max(other.max_positive),
            min_negative: self.min_negative.min(other.min_negative),
        }
    }

    pub fn adjoin_assign(&mut self, other: &Self) {
        *self = self.adjoin(other);
    }

    pub fn solve(&self) -> Length {
        self.max_positive + self.min_negative
    }
}
