/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use crate::context::LayoutContext;
use crate::dom_traversal::{Contents, NodeExt};
use crate::flow::BlockFormattingContext;
use crate::fragments::Fragment;
use crate::positioned::AbsolutelyPositionedFragment;
use crate::replaced::ReplacedContent;
use crate::sizing::{BoxContentSizes, ContentSizesRequest};
use crate::style_ext::DisplayInside;
use crate::ContainingBlock;
use servo_arc::Arc;
use std::convert::TryInto;
use style::properties::ComputedValues;
use style::values::computed::Length;

/// https://drafts.csswg.org/css-display/#independent-formatting-context
#[derive(Debug)]
pub(crate) struct IndependentFormattingContext {
    pub style: Arc<ComputedValues>,

    /// If it was requested during construction
    pub content_sizes: BoxContentSizes,

    contents: IndependentFormattingContextContents,
}

pub(crate) struct IndependentLayout {
    pub fragments: Vec<Fragment>,

    /// https://drafts.csswg.org/css2/visudet.html#root-height
    pub content_block_size: Length,
}

// Private so that code outside of this module cannot match variants.
// It should got through methods instead.
#[derive(Debug)]
enum IndependentFormattingContextContents {
    Flow(BlockFormattingContext),

    // Not called FC in specs, but behaves close enough
    Replaced(ReplacedContent),
    // Other layout modes go here
}

pub(crate) struct NonReplacedIFC<'a>(NonReplacedIFCKind<'a>);

enum NonReplacedIFCKind<'a> {
    Flow(&'a BlockFormattingContext),
}

impl IndependentFormattingContext {
    pub fn construct<'dom>(
        context: &LayoutContext,
        style: Arc<ComputedValues>,
        display_inside: DisplayInside,
        contents: Contents<impl NodeExt<'dom>>,
        content_sizes: ContentSizesRequest,
    ) -> Self {
        use self::IndependentFormattingContextContents as Contents;
        let (contents, content_sizes) = match contents.try_into() {
            Ok(non_replaced) => match display_inside {
                DisplayInside::Flow | DisplayInside::FlowRoot => {
                    let (bfc, box_content_sizes) = BlockFormattingContext::construct(
                        context,
                        &style,
                        non_replaced,
                        content_sizes,
                    );
                    (Contents::Flow(bfc), box_content_sizes)
                },
            },
            Err(replaced) => {
                // The `content_sizes` field is not used by layout code:
                (
                    Contents::Replaced(replaced),
                    BoxContentSizes::NoneWereRequested,
                )
            },
        };
        Self {
            style,
            contents,
            content_sizes,
        }
    }

    pub fn as_replaced(&self) -> Result<&ReplacedContent, NonReplacedIFC> {
        use self::IndependentFormattingContextContents as Contents;
        use self::NonReplacedIFC as NR;
        use self::NonReplacedIFCKind as Kind;
        match &self.contents {
            Contents::Replaced(r) => Ok(r),
            Contents::Flow(f) => Err(NR(Kind::Flow(f))),
        }
    }
}

impl<'a> NonReplacedIFC<'a> {
    pub fn layout(
        &self,
        layout_context: &LayoutContext,
        containing_block: &ContainingBlock,
        tree_rank: usize,
        absolutely_positioned_fragments: &mut Vec<AbsolutelyPositionedFragment<'a>>,
    ) -> IndependentLayout {
        match &self.0 {
            NonReplacedIFCKind::Flow(bfc) => bfc.layout(
                layout_context,
                containing_block,
                tree_rank,
                absolutely_positioned_fragments,
            ),
        }
    }
}
