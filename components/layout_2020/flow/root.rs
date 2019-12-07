/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use crate::context::LayoutContext;
use crate::display_list::IsContentful;
use crate::dom_traversal::{Contents, NodeExt};
use crate::flow::construct::ContainsFloats;
use crate::flow::float::FloatBox;
use crate::flow::{BlockContainer, BlockFormattingContext, BlockLevelBox};
use crate::formatting_contexts::IndependentFormattingContext;
use crate::fragments::Fragment;
use crate::geom;
use crate::geom::flow_relative::Vec2;
use crate::positioned::AbsolutelyPositionedBox;
use crate::replaced::ReplacedContent;
use crate::sizing::ContentSizesRequest;
use crate::style_ext::{Direction, Display, DisplayGeneratingBox, DisplayInside, WritingMode};
use crate::DefiniteContainingBlock;
use rayon::iter::{IntoParallelRefIterator, ParallelExtend, ParallelIterator};
use script_layout_interface::wrapper_traits::LayoutNode;
use servo_arc::Arc;
use style::values::computed::Length;
use style::Zero;
use style_traits::CSSPixel;

pub struct BoxTreeRoot(BlockFormattingContext);
pub struct FragmentTreeRoot(Vec<Fragment>);

impl BoxTreeRoot {
    pub fn construct<'dom, Node>(context: &LayoutContext, root_element: Node) -> Self
    where
        Node: 'dom + Copy + LayoutNode + Send + Sync,
    {
        let (contains_floats, boxes) = construct_for_root_element(&context, root_element);
        Self(BlockFormattingContext {
            contains_floats: contains_floats == ContainsFloats::Yes,
            contents: BlockContainer::BlockLevelBoxes(boxes),
        })
    }
}

fn construct_for_root_element<'dom>(
    context: &LayoutContext,
    root_element: impl NodeExt<'dom>,
) -> (ContainsFloats, Vec<Arc<BlockLevelBox>>) {
    let style = root_element.style(context);
    let replaced = ReplacedContent::for_element(root_element);
    let box_style = style.get_box();

    let display_inside = match Display::from(box_style.display) {
        Display::None => return (ContainsFloats::No, Vec::new()),
        Display::Contents if replaced.is_some() => {
            // 'display: contents' computes to 'none' for replaced elements
            return (ContainsFloats::No, Vec::new());
        },
        // https://drafts.csswg.org/css-display-3/#transformations
        Display::Contents => DisplayInside::Flow,
        // The root element is blockified, ignore DisplayOutside
        Display::GeneratingBox(DisplayGeneratingBox::OutsideInside { inside, .. }) => inside,
    };

    let contents = replaced.map_or(Contents::OfElement(root_element), Contents::Replaced);
    if box_style.position.is_absolutely_positioned() {
        (
            ContainsFloats::No,
            vec![Arc::new(BlockLevelBox::OutOfFlowAbsolutelyPositionedBox(
                AbsolutelyPositionedBox::construct(context, style, display_inside, contents),
            ))],
        )
    } else if box_style.float.is_floating() {
        (
            ContainsFloats::Yes,
            vec![Arc::new(BlockLevelBox::OutOfFlowFloatBox(
                FloatBox::construct(context, style, display_inside, contents),
            ))],
        )
    } else {
        (
            ContainsFloats::No,
            vec![Arc::new(BlockLevelBox::Independent(
                IndependentFormattingContext::construct(
                    context,
                    style,
                    display_inside,
                    contents,
                    ContentSizesRequest::None,
                ),
            ))],
        )
    }
}

impl BoxTreeRoot {
    pub fn layout(
        &self,
        layout_context: &LayoutContext,
        viewport: geom::Size<CSSPixel>,
    ) -> FragmentTreeRoot {
        let initial_containing_block = DefiniteContainingBlock {
            size: Vec2 {
                inline: Length::new(viewport.width),
                block: Length::new(viewport.height),
            },
            // FIXME: use the document’s mode:
            // https://drafts.csswg.org/css-writing-modes/#principal-flow
            mode: (WritingMode::HorizontalTb, Direction::Ltr),
        };

        let dummy_tree_rank = 0;
        let mut absolutely_positioned_fragments = vec![];
        let mut independent_layout = self.0.layout(
            layout_context,
            &(&initial_containing_block).into(),
            dummy_tree_rank,
            &mut absolutely_positioned_fragments,
        );

        independent_layout.fragments.par_extend(
            absolutely_positioned_fragments
                .par_iter()
                .map(|a| a.layout(layout_context, &initial_containing_block)),
        );
        FragmentTreeRoot(independent_layout.fragments)
    }
}

impl FragmentTreeRoot {
    pub fn build_display_list(
        &self,
        builder: &mut crate::display_list::DisplayListBuilder,
        viewport_size: webrender_api::units::LayoutSize,
    ) -> IsContentful {
        let containing_block = geom::physical::Rect {
            top_left: geom::physical::Vec2 {
                x: Length::zero(),
                y: Length::zero(),
            },
            size: geom::physical::Vec2 {
                x: Length::new(viewport_size.width),
                y: Length::new(viewport_size.height),
            },
        };
        let mut is_contentful = IsContentful(false);
        for fragment in &self.0 {
            fragment.build_display_list(builder, &mut is_contentful, &containing_block)
        }
        is_contentful
    }
}
