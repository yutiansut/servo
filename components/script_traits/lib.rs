/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

//! This module contains traits in script used generically in the rest of Servo.
//! The traits are here instead of in script so that these modules won't have
//! to depend on script.

#![deny(missing_docs)]
#![deny(unsafe_code)]

#[macro_use]
extern crate malloc_size_of;
#[macro_use]
extern crate malloc_size_of_derive;
#[macro_use]
extern crate serde;

mod script_msg;
pub mod transferable;
pub mod webdriver_msg;

use crate::transferable::MessagePortImpl;
use crate::webdriver_msg::{LoadStatus, WebDriverScriptCommand};
use bluetooth_traits::BluetoothRequest;
use canvas_traits::webgl::WebGLPipeline;
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use devtools_traits::{DevtoolScriptControlMsg, ScriptToDevtoolsControlMsg, WorkerId};
use embedder_traits::{Cursor, EventLoopWaker};
use euclid::{default::Point2D, Length, Rect, Scale, Size2D, UnknownUnit, Vector2D};
use gfx_traits::Epoch;
use http::HeaderMap;
use hyper::Method;
use ipc_channel::ipc::{self, IpcReceiver, IpcSender};
use ipc_channel::Error as IpcError;
use keyboard_types::webdriver::Event as WebDriverInputEvent;
use keyboard_types::{CompositionEvent, KeyboardEvent};
use libc::c_void;
use log::warn;
use media::WindowGLContext;
use msg::constellation_msg::BackgroundHangMonitorRegister;
use msg::constellation_msg::{BrowsingContextId, HistoryStateId, MessagePortId, PipelineId};
use msg::constellation_msg::{PipelineNamespaceId, TopLevelBrowsingContextId, TraversalDirection};
use net_traits::image::base::Image;
use net_traits::image_cache::ImageCache;
use net_traits::request::Referrer;
use net_traits::storage_thread::StorageType;
use net_traits::{FetchResponseMsg, ReferrerPolicy, ResourceThreads};
use pixels::PixelFormat;
use profile_traits::mem;
use profile_traits::time as profile_time;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use servo_atoms::Atom;
use servo_url::ImmutableOrigin;
use servo_url::ServoUrl;
use std::borrow::Cow;
use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;
use style_traits::CSSPixel;
use style_traits::SpeculativePainter;
use webgpu::WebGPU;
use webrender_api::units::{
    DeviceIntSize, DevicePixel, LayoutPixel, LayoutPoint, LayoutSize, WorldPoint,
};
use webrender_api::{BuiltDisplayList, DocumentId, ExternalScrollId, ImageKey, ScrollClamping};
use webrender_api::{BuiltDisplayListDescriptor, HitTestFlags, HitTestResult, ResourceUpdate};
use webvr_traits::{WebVREvent, WebVRMsg};

pub use crate::script_msg::{
    DOMMessage, HistoryEntryReplacement, SWManagerMsg, SWManagerSenders, ScopeThings,
    ServiceWorkerMsg,
};
pub use crate::script_msg::{
    EventResult, IFrameSize, IFrameSizeMsg, LayoutMsg, LogEntry, ScriptMsg,
};

/// The address of a node. Layout sends these back. They must be validated via
/// `from_untrusted_node_address` before they can be used, because we do not trust layout.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct UntrustedNodeAddress(pub *const c_void);

malloc_size_of_is_0!(UntrustedNodeAddress);

#[allow(unsafe_code)]
unsafe impl Send for UntrustedNodeAddress {}

impl Serialize for UntrustedNodeAddress {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        (self.0 as usize).serialize(s)
    }
}

impl<'de> Deserialize<'de> for UntrustedNodeAddress {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<UntrustedNodeAddress, D::Error> {
        let value: usize = Deserialize::deserialize(d)?;
        Ok(UntrustedNodeAddress::from_id(value))
    }
}

impl UntrustedNodeAddress {
    /// Creates an `UntrustedNodeAddress` from the given pointer address value.
    #[inline]
    pub fn from_id(id: usize) -> UntrustedNodeAddress {
        UntrustedNodeAddress(id as *const c_void)
    }
}

/// Messages sent to the layout thread from the constellation and/or compositor.
#[derive(Debug, Deserialize, Serialize)]
pub enum LayoutControlMsg {
    /// Requests that this layout thread exit.
    ExitNow,
    /// Requests the current epoch (layout counter) from this layout.
    GetCurrentEpoch(IpcSender<Epoch>),
    /// Asks layout to run another step in its animation.
    TickAnimations,
    /// Tells layout about the new scrolling offsets of each scrollable stacking context.
    SetScrollStates(Vec<ScrollState>),
    /// Requests the current load state of Web fonts. `true` is returned if fonts are still loading
    /// and `false` is returned if all fonts have loaded.
    GetWebFontLoadState(IpcSender<bool>),
    /// Send the paint time for a specific epoch to the layout thread.
    PaintMetric(Epoch, u64),
}

/// The origin where a given load was initiated.
/// Useful for origin checks, for example before evaluation a JS URL.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum LoadOrigin {
    /// A load originating in the constellation.
    Constellation,
    /// A load originating in webdriver.
    WebDriver,
    /// A load originating in script.
    Script(ImmutableOrigin),
}

/// can be passed to `LoadUrl` to load a page with GET/POST
/// parameters or headers
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LoadData {
    /// The origin where the load started.
    pub load_origin: LoadOrigin,
    /// The URL.
    pub url: ServoUrl,
    /// The creator pipeline id if this is an about:blank load.
    pub creator_pipeline_id: Option<PipelineId>,
    /// The method.
    #[serde(
        deserialize_with = "::hyper_serde::deserialize",
        serialize_with = "::hyper_serde::serialize"
    )]
    pub method: Method,
    /// The headers.
    #[serde(
        deserialize_with = "::hyper_serde::deserialize",
        serialize_with = "::hyper_serde::serialize"
    )]
    pub headers: HeaderMap,
    /// The data.
    pub data: Option<Vec<u8>>,
    /// The result of evaluating a javascript scheme url.
    pub js_eval_result: Option<JsEvalResult>,
    /// The referrer.
    pub referrer: Option<Referrer>,
    /// The referrer policy.
    pub referrer_policy: Option<ReferrerPolicy>,

    /// The source to use instead of a network response for a srcdoc document.
    pub srcdoc: String,
}

/// The result of evaluating a javascript scheme url.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum JsEvalResult {
    /// The js evaluation had a non-string result, 204 status code.
    /// <https://html.spec.whatwg.org/multipage/#navigate> 12.11
    NoContent,
    /// The js evaluation had a string result.
    Ok(Vec<u8>),
}

impl LoadData {
    /// Create a new `LoadData` object.
    pub fn new(
        load_origin: LoadOrigin,
        url: ServoUrl,
        creator_pipeline_id: Option<PipelineId>,
        referrer: Option<Referrer>,
        referrer_policy: Option<ReferrerPolicy>,
    ) -> LoadData {
        LoadData {
            load_origin,
            url: url,
            creator_pipeline_id: creator_pipeline_id,
            method: Method::GET,
            headers: HeaderMap::new(),
            data: None,
            js_eval_result: None,
            referrer: referrer,
            referrer_policy: referrer_policy,
            srcdoc: "".to_string(),
        }
    }
}

/// The initial data required to create a new layout attached to an existing script thread.
#[derive(Debug, Deserialize, Serialize)]
pub struct NewLayoutInfo {
    /// The ID of the parent pipeline and frame type, if any.
    /// If `None`, this is a root pipeline.
    pub parent_info: Option<PipelineId>,
    /// Id of the newly-created pipeline.
    pub new_pipeline_id: PipelineId,
    /// Id of the browsing context associated with this pipeline.
    pub browsing_context_id: BrowsingContextId,
    /// Id of the top-level browsing context associated with this pipeline.
    pub top_level_browsing_context_id: TopLevelBrowsingContextId,
    /// Id of the opener, if any
    pub opener: Option<BrowsingContextId>,
    /// Network request data which will be initiated by the script thread.
    pub load_data: LoadData,
    /// Information about the initial window size.
    pub window_size: WindowSizeData,
    /// A port on which layout can receive messages from the pipeline.
    pub pipeline_port: IpcReceiver<LayoutControlMsg>,
}

/// When a pipeline is closed, should its browsing context be discarded too?
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub enum DiscardBrowsingContext {
    /// Discard the browsing context
    Yes,
    /// Don't discard the browsing context
    No,
}

/// Is a document fully active, active or inactive?
/// A document is active if it is the current active document in its session history,
/// it is fuly active if it is active and all of its ancestors are active,
/// and it is inactive otherwise.
///
/// * <https://html.spec.whatwg.org/multipage/#active-document>
/// * <https://html.spec.whatwg.org/multipage/#fully-active>
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, MallocSizeOf, PartialEq, Serialize)]
pub enum DocumentActivity {
    /// An inactive document
    Inactive,
    /// An active but not fully active document
    Active,
    /// A fully active document
    FullyActive,
}

/// Type of recorded progressive web metric
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub enum ProgressiveWebMetricType {
    /// Time to first Paint
    FirstPaint,
    /// Time to first contentful paint
    FirstContentfulPaint,
    /// Time to interactive
    TimeToInteractive,
}

/// The reason why the pipeline id of an iframe is being updated.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, MallocSizeOf, PartialEq, Serialize)]
pub enum UpdatePipelineIdReason {
    /// The pipeline id is being updated due to a navigation.
    Navigation,
    /// The pipeline id is being updated due to a history traversal.
    Traversal,
}

/// Messages sent from the constellation or layout to the script thread.
#[derive(Deserialize, Serialize)]
pub enum ConstellationControlMsg {
    /// Takes the associated window proxy out of "delaying-load-events-mode",
    /// used if a scheduled navigated was refused by the embedder.
    /// https://html.spec.whatwg.org/multipage/#delaying-load-events-mode
    StopDelayingLoadEventsMode(PipelineId),
    /// Sends the final response to script thread for fetching after all redirections
    /// have been resolved
    NavigationResponse(PipelineId, FetchResponseMsg),
    /// Gives a channel and ID to a layout thread, as well as the ID of that layout's parent
    AttachLayout(NewLayoutInfo),
    /// Window resized.  Sends a DOM event eventually, but first we combine events.
    Resize(PipelineId, WindowSizeData, WindowSizeType),
    /// Notifies script that window has been resized but to not take immediate action.
    ResizeInactive(PipelineId, WindowSizeData),
    /// Window switched from fullscreen mode.
    ExitFullScreen(PipelineId),
    /// Notifies the script that the document associated with this pipeline should 'unload'.
    UnloadDocument(PipelineId),
    /// Notifies the script that a pipeline should be closed.
    ExitPipeline(PipelineId, DiscardBrowsingContext),
    /// Notifies the script that the whole thread should be closed.
    ExitScriptThread,
    /// Sends a DOM event.
    SendEvent(PipelineId, CompositorEvent),
    /// Notifies script of the viewport.
    Viewport(PipelineId, Rect<f32, UnknownUnit>),
    /// Notifies script of a new set of scroll offsets.
    SetScrollState(
        PipelineId,
        Vec<(UntrustedNodeAddress, Vector2D<f32, LayoutPixel>)>,
    ),
    /// Requests that the script thread immediately send the constellation the title of a pipeline.
    GetTitle(PipelineId),
    /// Notifies script thread of a change to one of its document's activity
    SetDocumentActivity(PipelineId, DocumentActivity),
    /// Notifies script thread whether frame is visible
    ChangeFrameVisibilityStatus(PipelineId, bool),
    /// Notifies script thread that frame visibility change is complete
    /// PipelineId is for the parent, BrowsingContextId is for the nested browsing context
    NotifyVisibilityChange(PipelineId, BrowsingContextId, bool),
    /// Notifies script thread that a url should be loaded in this iframe.
    /// PipelineId is for the parent, BrowsingContextId is for the nested browsing context
    NavigateIframe(
        PipelineId,
        BrowsingContextId,
        LoadData,
        HistoryEntryReplacement,
    ),
    /// Post a message to a given window.
    PostMessage {
        /// The target of the message.
        target: PipelineId,
        /// The source of the message.
        source: PipelineId,
        /// The top level browsing context associated with the source pipeline.
        source_browsing_context: TopLevelBrowsingContextId,
        /// The expected origin of the target.
        target_origin: Option<ImmutableOrigin>,
        /// The source origin of the message.
        /// https://html.spec.whatwg.org/multipage/#dom-messageevent-origin
        source_origin: ImmutableOrigin,
        /// The data to be posted.
        data: StructuredSerializedData,
    },
    /// Updates the current pipeline ID of a given iframe.
    /// First PipelineId is for the parent, second is the new PipelineId for the frame.
    UpdatePipelineId(
        PipelineId,
        BrowsingContextId,
        TopLevelBrowsingContextId,
        PipelineId,
        UpdatePipelineIdReason,
    ),
    /// Updates the history state and url of a given pipeline.
    UpdateHistoryState(PipelineId, Option<HistoryStateId>, ServoUrl),
    /// Removes inaccesible history states.
    RemoveHistoryStates(PipelineId, Vec<HistoryStateId>),
    /// Set an iframe to be focused. Used when an element in an iframe gains focus.
    /// PipelineId is for the parent, BrowsingContextId is for the nested browsing context
    FocusIFrame(PipelineId, BrowsingContextId),
    /// Passes a webdriver command to the script thread for execution
    WebDriverScriptCommand(PipelineId, WebDriverScriptCommand),
    /// Notifies script thread that all animations are done
    TickAllAnimations(PipelineId),
    /// Notifies the script thread of a transition end
    TransitionEnd(UntrustedNodeAddress, String, f64),
    /// Notifies the script thread that a new Web font has been loaded, and thus the page should be
    /// reflowed.
    WebFontLoaded(PipelineId),
    /// Cause a `load` event to be dispatched at the appropriate iframe element.
    DispatchIFrameLoadEvent {
        /// The frame that has been marked as loaded.
        target: BrowsingContextId,
        /// The pipeline that contains a frame loading the target pipeline.
        parent: PipelineId,
        /// The pipeline that has completed loading.
        child: PipelineId,
    },
    /// Cause a `storage` event to be dispatched at the appropriate window.
    /// The strings are key, old value and new value.
    DispatchStorageEvent(
        PipelineId,
        StorageType,
        ServoUrl,
        Option<String>,
        Option<String>,
        Option<String>,
    ),
    /// Report an error from a CSS parser for the given pipeline
    ReportCSSError(PipelineId, String, u32, u32, String),
    /// Reload the given page.
    Reload(PipelineId),
    /// Notifies the script thread of WebVR events.
    WebVREvents(PipelineId, Vec<WebVREvent>),
    /// Notifies the script thread about a new recorded paint metric.
    PaintMetric(PipelineId, ProgressiveWebMetricType, u64),
    /// Notifies the media session about a user requested media session action.
    MediaSessionAction(PipelineId, MediaSessionActionType),
}

impl fmt::Debug for ConstellationControlMsg {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        use self::ConstellationControlMsg::*;
        let variant = match *self {
            StopDelayingLoadEventsMode(..) => "StopDelayingLoadsEventMode",
            NavigationResponse(..) => "NavigationResponse",
            AttachLayout(..) => "AttachLayout",
            Resize(..) => "Resize",
            ResizeInactive(..) => "ResizeInactive",
            UnloadDocument(..) => "UnloadDocument",
            ExitPipeline(..) => "ExitPipeline",
            ExitScriptThread => "ExitScriptThread",
            SendEvent(..) => "SendEvent",
            Viewport(..) => "Viewport",
            SetScrollState(..) => "SetScrollState",
            GetTitle(..) => "GetTitle",
            SetDocumentActivity(..) => "SetDocumentActivity",
            ChangeFrameVisibilityStatus(..) => "ChangeFrameVisibilityStatus",
            NotifyVisibilityChange(..) => "NotifyVisibilityChange",
            NavigateIframe(..) => "NavigateIframe",
            PostMessage { .. } => "PostMessage",
            UpdatePipelineId(..) => "UpdatePipelineId",
            UpdateHistoryState(..) => "UpdateHistoryState",
            RemoveHistoryStates(..) => "RemoveHistoryStates",
            FocusIFrame(..) => "FocusIFrame",
            WebDriverScriptCommand(..) => "WebDriverScriptCommand",
            TickAllAnimations(..) => "TickAllAnimations",
            TransitionEnd(..) => "TransitionEnd",
            WebFontLoaded(..) => "WebFontLoaded",
            DispatchIFrameLoadEvent { .. } => "DispatchIFrameLoadEvent",
            DispatchStorageEvent(..) => "DispatchStorageEvent",
            ReportCSSError(..) => "ReportCSSError",
            Reload(..) => "Reload",
            WebVREvents(..) => "WebVREvents",
            PaintMetric(..) => "PaintMetric",
            ExitFullScreen(..) => "ExitFullScreen",
            MediaSessionAction(..) => "MediaSessionAction",
        };
        write!(formatter, "ConstellationControlMsg::{}", variant)
    }
}

/// Used to determine if a script has any pending asynchronous activity.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub enum DocumentState {
    /// The document has been loaded and is idle.
    Idle,
    /// The document is either loading or waiting on an event.
    Pending,
}

/// For a given pipeline, whether any animations are currently running
/// and any animation callbacks are queued
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum AnimationState {
    /// Animations are active but no callbacks are queued
    AnimationsPresent,
    /// Animations are active and callbacks are queued
    AnimationCallbacksPresent,
    /// No animations are active and no callbacks are queued
    NoAnimationsPresent,
    /// No animations are active but callbacks are queued
    NoAnimationCallbacksPresent,
}

/// The type of input represented by a multi-touch event.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub enum TouchEventType {
    /// A new touch point came in contact with the screen.
    Down,
    /// An existing touch point changed location.
    Move,
    /// A touch point was removed from the screen.
    Up,
    /// The system stopped tracking a touch point.
    Cancel,
}

/// An opaque identifier for a touch point.
///
/// <http://w3c.github.io/touch-events/#widl-Touch-identifier>
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TouchId(pub i32);

/// The mouse button involved in the event.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub enum MouseButton {
    /// The left mouse button.
    Left = 1,
    /// The right mouse button.
    Right = 2,
    /// The middle mouse button.
    Middle = 4,
}

/// The types of mouse events
#[derive(Debug, Deserialize, MallocSizeOf, Serialize)]
pub enum MouseEventType {
    /// Mouse button clicked
    Click,
    /// Mouse button down
    MouseDown,
    /// Mouse button up
    MouseUp,
}

/// Mode to measure WheelDelta floats in
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub enum WheelMode {
    /// Delta values are specified in pixels
    DeltaPixel = 0x00,
    /// Delta values are specified in lines
    DeltaLine = 0x01,
    /// Delta values are specified in pages
    DeltaPage = 0x02,
}

/// The Wheel event deltas in every direction
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub struct WheelDelta {
    /// Delta in the left/right direction
    pub x: f64,
    /// Delta in the up/down direction
    pub y: f64,
    /// Delta in the direction going into/out of the screen
    pub z: f64,
    /// Mode to measure the floats in
    pub mode: WheelMode,
}

/// Events from the compositor that the script thread needs to know about
#[derive(Debug, Deserialize, Serialize)]
pub enum CompositorEvent {
    /// The window was resized.
    ResizeEvent(WindowSizeData, WindowSizeType),
    /// A mouse button state changed.
    MouseButtonEvent(
        MouseEventType,
        MouseButton,
        Point2D<f32>,
        Option<UntrustedNodeAddress>,
        Option<Point2D<f32>>,
        // Bitmask of MouseButton values representing the currently pressed buttons
        u16,
    ),
    /// The mouse was moved over a point (or was moved out of the recognizable region).
    MouseMoveEvent(
        Option<Point2D<f32>>,
        Option<UntrustedNodeAddress>,
        // Bitmask of MouseButton values representing the currently pressed buttons
        u16,
    ),
    /// A touch event was generated with a touch ID and location.
    TouchEvent(
        TouchEventType,
        TouchId,
        Point2D<f32>,
        Option<UntrustedNodeAddress>,
    ),
    /// A wheel event was generated with a delta in the X, Y, and/or Z directions
    WheelEvent(WheelDelta, Point2D<f32>, Option<UntrustedNodeAddress>),
    /// A key was pressed.
    KeyboardEvent(KeyboardEvent),
    /// An event from the IME is dispatched.
    CompositionEvent(CompositionEvent),
}

/// Requests a TimerEvent-Message be sent after the given duration.
#[derive(Debug, Deserialize, Serialize)]
pub struct TimerEventRequest(
    pub IpcSender<TimerEvent>,
    pub TimerSource,
    pub TimerEventId,
    pub MsDuration,
);

/// The message used to send a request to the timer scheduler.
#[derive(Debug, Deserialize, Serialize)]
pub struct TimerSchedulerMsg(pub TimerEventRequest);

/// Notifies the script thread to fire due timers.
/// `TimerSource` must be `FromWindow` when dispatched to `ScriptThread` and
/// must be `FromWorker` when dispatched to a `DedicatedGlobalWorkerScope`
#[derive(Debug, Deserialize, Serialize)]
pub struct TimerEvent(pub TimerSource, pub TimerEventId);

/// Describes the thread that requested the TimerEvent.
#[derive(Clone, Copy, Debug, Deserialize, MallocSizeOf, Serialize)]
pub enum TimerSource {
    /// The event was requested from a window (ScriptThread).
    FromWindow(PipelineId),
    /// The event was requested from a worker (DedicatedGlobalWorkerScope).
    FromWorker,
}

/// The id to be used for a `TimerEvent` is defined by the corresponding `TimerEventRequest`.
#[derive(Clone, Copy, Debug, Deserialize, Eq, MallocSizeOf, PartialEq, Serialize)]
pub struct TimerEventId(pub u32);

/// Unit of measurement.
#[derive(Clone, Copy, MallocSizeOf)]
pub enum Milliseconds {}
/// Unit of measurement.
#[derive(Clone, Copy, MallocSizeOf)]
pub enum Nanoseconds {}

/// Amount of milliseconds.
pub type MsDuration = Length<u64, Milliseconds>;
/// Amount of nanoseconds.
pub type NsDuration = Length<u64, Nanoseconds>;

/// Returns the duration since an unspecified epoch measured in ms.
pub fn precise_time_ms() -> MsDuration {
    Length::new(time::precise_time_ns() / (1000 * 1000))
}

/// Data needed to construct a script thread.
///
/// NB: *DO NOT* add any Senders or Receivers here! pcwalton will have to rewrite your code if you
/// do! Use IPC senders and receivers instead.
pub struct InitialScriptState {
    /// The ID of the pipeline with which this script thread is associated.
    pub id: PipelineId,
    /// The subpage ID of this pipeline to create in its pipeline parent.
    /// If `None`, this is the root.
    pub parent_info: Option<PipelineId>,
    /// The ID of the browsing context this script is part of.
    pub browsing_context_id: BrowsingContextId,
    /// The ID of the top-level browsing context this script is part of.
    pub top_level_browsing_context_id: TopLevelBrowsingContextId,
    /// The ID of the opener, if any.
    pub opener: Option<BrowsingContextId>,
    /// A channel with which messages can be sent to us (the script thread).
    pub control_chan: IpcSender<ConstellationControlMsg>,
    /// A port on which messages sent by the constellation to script can be received.
    pub control_port: IpcReceiver<ConstellationControlMsg>,
    /// A channel on which messages can be sent to the constellation from script.
    pub script_to_constellation_chan: ScriptToConstellationChan,
    /// A handle to register script-(and associated layout-)threads for hang monitoring.
    pub background_hang_monitor_register: Box<dyn BackgroundHangMonitorRegister>,
    /// A sender for the layout thread to communicate to the constellation.
    pub layout_to_constellation_chan: IpcSender<LayoutMsg>,
    /// A channel to schedule timer events.
    pub scheduler_chan: IpcSender<TimerSchedulerMsg>,
    /// A channel to the resource manager thread.
    pub resource_threads: ResourceThreads,
    /// A channel to the bluetooth thread.
    pub bluetooth_thread: IpcSender<BluetoothRequest>,
    /// The image cache for this script thread.
    pub image_cache: Arc<dyn ImageCache>,
    /// A channel to the time profiler thread.
    pub time_profiler_chan: profile_traits::time::ProfilerChan,
    /// A channel to the memory profiler thread.
    pub mem_profiler_chan: mem::ProfilerChan,
    /// A channel to the developer tools, if applicable.
    pub devtools_chan: Option<IpcSender<ScriptToDevtoolsControlMsg>>,
    /// Information about the initial window size.
    pub window_size: WindowSizeData,
    /// The ID of the pipeline namespace for this script thread.
    pub pipeline_namespace_id: PipelineNamespaceId,
    /// A ping will be sent on this channel once the script thread shuts down.
    pub content_process_shutdown_chan: Sender<()>,
    /// A channel to the WebGL thread used in this pipeline.
    pub webgl_chan: Option<WebGLPipeline>,
    /// A channel to the WebGPU threads.
    pub webgpu: Option<WebGPU>,
    /// A channel to the webvr thread, if available.
    pub webvr_chan: Option<IpcSender<WebVRMsg>>,
    /// The XR device registry
    pub webxr_registry: webxr_api::Registry,
    /// The Webrender document ID associated with this thread.
    pub webrender_document: DocumentId,
    /// FIXME(victor): The Webrender API sender in this constellation's pipeline
    pub webrender_api_sender: WebrenderIpcSender,
    /// Flag to indicate if the layout thread is busy handling a request.
    pub layout_is_busy: Arc<AtomicBool>,
    /// Application window's GL Context for Media player
    pub player_context: WindowGLContext,
    /// Mechanism to force the compositor to process events.
    pub event_loop_waker: Option<Box<dyn EventLoopWaker>>,
}

/// This trait allows creating a `ScriptThread` without depending on the `script`
/// crate.
pub trait ScriptThreadFactory {
    /// Type of message sent from script to layout.
    type Message;
    /// Create a `ScriptThread`.
    fn create(
        state: InitialScriptState,
        load_data: LoadData,
        profile_script_events: bool,
        print_pwm: bool,
        relayout_event: bool,
        prepare_for_screenshot: bool,
        unminify_js: bool,
        userscripts_path: Option<String>,
        headless: bool,
        replace_surrogates: bool,
        user_agent: Cow<'static, str>,
    ) -> (Sender<Self::Message>, Receiver<Self::Message>);
}

/// Whether the sandbox attribute is present for an iframe element
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum IFrameSandboxState {
    /// Sandbox attribute is present
    IFrameSandboxed,
    /// Sandbox attribute is not present
    IFrameUnsandboxed,
}

/// Specifies the information required to load an auxiliary browsing context.
#[derive(Debug, Deserialize, Serialize)]
pub struct AuxiliaryBrowsingContextLoadInfo {
    /// Load data containing the url to load
    pub load_data: LoadData,
    /// The pipeline opener browsing context.
    pub opener_pipeline_id: PipelineId,
    /// The new top-level ID for the auxiliary.
    pub new_top_level_browsing_context_id: TopLevelBrowsingContextId,
    /// The new browsing context ID.
    pub new_browsing_context_id: BrowsingContextId,
    /// The new pipeline ID for the auxiliary.
    pub new_pipeline_id: PipelineId,
}

/// Specifies the information required to load an iframe.
#[derive(Debug, Deserialize, Serialize)]
pub struct IFrameLoadInfo {
    /// Pipeline ID of the parent of this iframe
    pub parent_pipeline_id: PipelineId,
    /// The ID for this iframe's nested browsing context.
    pub browsing_context_id: BrowsingContextId,
    /// The ID for the top-level ancestor browsing context of this iframe's nested browsing context.
    pub top_level_browsing_context_id: TopLevelBrowsingContextId,
    /// The new pipeline ID that the iframe has generated.
    pub new_pipeline_id: PipelineId,
    ///  Whether this iframe should be considered private
    pub is_private: bool,
    /// Wether this load should replace the current entry (reload). If true, the current
    /// entry will be replaced instead of a new entry being added.
    pub replace: HistoryEntryReplacement,
}

/// Specifies the information required to load a URL in an iframe.
#[derive(Debug, Deserialize, Serialize)]
pub struct IFrameLoadInfoWithData {
    /// The information required to load an iframe.
    pub info: IFrameLoadInfo,
    /// Load data containing the url to load
    pub load_data: LoadData,
    /// The old pipeline ID for this iframe, if a page was previously loaded.
    pub old_pipeline_id: Option<PipelineId>,
    /// Sandbox type of this iframe
    pub sandbox: IFrameSandboxState,
    /// The initial viewport size for this iframe.
    pub window_size: WindowSizeData,
}

/// Specifies whether the script or layout thread needs to be ticked for animation.
#[derive(Debug, Deserialize, Serialize)]
pub enum AnimationTickType {
    /// The script thread.
    Script,
    /// The layout thread.
    Layout,
}

/// The scroll state of a stacking context.
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct ScrollState {
    /// The ID of the scroll root.
    pub scroll_id: ExternalScrollId,
    /// The scrolling offset of this stacking context.
    pub scroll_offset: Vector2D<f32, LayoutPixel>,
}

/// Data about the window size.
#[derive(Clone, Copy, Debug, Deserialize, MallocSizeOf, PartialEq, Serialize)]
pub struct WindowSizeData {
    /// The size of the initial layout viewport, before parsing an
    /// <http://www.w3.org/TR/css-device-adapt/#initial-viewport>
    pub initial_viewport: Size2D<f32, CSSPixel>,

    /// The resolution of the window in dppx, not including any "pinch zoom" factor.
    pub device_pixel_ratio: Scale<f32, CSSPixel, DevicePixel>,
}

/// The type of window size change.
#[derive(Clone, Copy, Debug, Deserialize, Eq, MallocSizeOf, PartialEq, Serialize)]
pub enum WindowSizeType {
    /// Initial load.
    Initial,
    /// Window resize.
    Resize,
}

/// Messages to the constellation originating from the WebDriver server.
#[derive(Debug, Deserialize, Serialize)]
pub enum WebDriverCommandMsg {
    /// Get the window size.
    GetWindowSize(TopLevelBrowsingContextId, IpcSender<WindowSizeData>),
    /// Load a URL in the top-level browsing context with the given ID.
    LoadUrl(TopLevelBrowsingContextId, LoadData, IpcSender<LoadStatus>),
    /// Refresh the top-level browsing context with the given ID.
    Refresh(TopLevelBrowsingContextId, IpcSender<LoadStatus>),
    /// Pass a webdriver command to the script thread of the current pipeline
    /// of a browsing context.
    ScriptCommand(BrowsingContextId, WebDriverScriptCommand),
    /// Act as if keys were pressed in the browsing context with the given ID.
    SendKeys(BrowsingContextId, Vec<WebDriverInputEvent>),
    /// Act as if keys were pressed or release in the browsing context with the given ID.
    KeyboardAction(BrowsingContextId, KeyboardEvent),
    /// Act as if the mouse was clicked in the browsing context with the given ID.
    MouseButtonAction(MouseEventType, MouseButton, f32, f32),
    /// Act as if the mouse was moved in the browsing context with the given ID.
    MouseMoveAction(f32, f32),
    /// Set the window size.
    SetWindowSize(
        TopLevelBrowsingContextId,
        DeviceIntSize,
        IpcSender<WindowSizeData>,
    ),
    /// Take a screenshot of the window.
    TakeScreenshot(
        TopLevelBrowsingContextId,
        Option<Rect<f32, CSSPixel>>,
        IpcSender<Option<Image>>,
    ),
}

/// Messages to the constellation.
#[derive(Deserialize, Serialize)]
pub enum ConstellationMsg {
    /// Exit the constellation.
    Exit,
    /// Request that the constellation send the BrowsingContextId corresponding to the document
    /// with the provided pipeline id
    GetBrowsingContext(PipelineId, IpcSender<Option<BrowsingContextId>>),
    /// Request that the constellation send the current pipeline id for the provided
    /// browsing context id, over a provided channel.
    GetPipeline(BrowsingContextId, IpcSender<Option<PipelineId>>),
    /// Request that the constellation send the current focused top-level browsing context id,
    /// over a provided channel.
    GetFocusTopLevelBrowsingContext(IpcSender<Option<TopLevelBrowsingContextId>>),
    /// Query the constellation to see if the current compositor output is stable
    IsReadyToSaveImage(HashMap<PipelineId, Epoch>),
    /// Inform the constellation of a key event.
    Keyboard(KeyboardEvent),
    /// Whether to allow script to navigate.
    AllowNavigationResponse(PipelineId, bool),
    /// Request to load a page.
    LoadUrl(TopLevelBrowsingContextId, ServoUrl),
    /// Request to traverse the joint session history of the provided browsing context.
    TraverseHistory(TopLevelBrowsingContextId, TraversalDirection),
    /// Inform the constellation of a window being resized.
    WindowSize(
        Option<TopLevelBrowsingContextId>,
        WindowSizeData,
        WindowSizeType,
    ),
    /// Requests that the constellation instruct layout to begin a new tick of the animation.
    TickAnimation(PipelineId, AnimationTickType),
    /// Dispatch a webdriver command
    WebDriverCommand(WebDriverCommandMsg),
    /// Reload a top-level browsing context.
    Reload(TopLevelBrowsingContextId),
    /// A log entry, with the top-level browsing context id and thread name
    LogEntry(Option<TopLevelBrowsingContextId>, Option<String>, LogEntry),
    /// Dispatch WebVR events to the subscribed script threads.
    WebVREvents(Vec<PipelineId>, Vec<WebVREvent>),
    /// Create a new top level browsing context.
    NewBrowser(ServoUrl, TopLevelBrowsingContextId),
    /// Close a top level browsing context.
    CloseBrowser(TopLevelBrowsingContextId),
    /// Panic a top level browsing context.
    SendError(Option<TopLevelBrowsingContextId>, String),
    /// Make browser visible.
    SelectBrowser(TopLevelBrowsingContextId),
    /// Forward an event to the script task of the given pipeline.
    ForwardEvent(PipelineId, CompositorEvent),
    /// Requesting a change to the onscreen cursor.
    SetCursor(Cursor),
    /// Enable the sampling profiler, with a given sampling rate and max total sampling duration.
    EnableProfiler(Duration, Duration),
    /// Disable the sampling profiler.
    DisableProfiler,
    /// Request to exit from fullscreen mode
    ExitFullScreen(TopLevelBrowsingContextId),
    /// Media session action.
    MediaSessionAction(MediaSessionActionType),
}

impl fmt::Debug for ConstellationMsg {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        use self::ConstellationMsg::*;
        let variant = match *self {
            Exit => "Exit",
            GetBrowsingContext(..) => "GetBrowsingContext",
            GetPipeline(..) => "GetPipeline",
            GetFocusTopLevelBrowsingContext(..) => "GetFocusTopLevelBrowsingContext",
            IsReadyToSaveImage(..) => "IsReadyToSaveImage",
            Keyboard(..) => "Keyboard",
            AllowNavigationResponse(..) => "AllowNavigationResponse",
            LoadUrl(..) => "LoadUrl",
            TraverseHistory(..) => "TraverseHistory",
            WindowSize(..) => "WindowSize",
            TickAnimation(..) => "TickAnimation",
            WebDriverCommand(..) => "WebDriverCommand",
            Reload(..) => "Reload",
            LogEntry(..) => "LogEntry",
            WebVREvents(..) => "WebVREvents",
            NewBrowser(..) => "NewBrowser",
            CloseBrowser(..) => "CloseBrowser",
            SendError(..) => "SendError",
            SelectBrowser(..) => "SelectBrowser",
            ForwardEvent(..) => "ForwardEvent",
            SetCursor(..) => "SetCursor",
            EnableProfiler(..) => "EnableProfiler",
            DisableProfiler => "DisableProfiler",
            ExitFullScreen(..) => "ExitFullScreen",
            MediaSessionAction(..) => "MediaSessionAction",
        };
        write!(formatter, "ConstellationMsg::{}", variant)
    }
}

/// Resources required by workerglobalscopes
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct WorkerGlobalScopeInit {
    /// Chan to a resource thread
    pub resource_threads: ResourceThreads,
    /// Chan to the memory profiler
    pub mem_profiler_chan: mem::ProfilerChan,
    /// Chan to the time profiler
    pub time_profiler_chan: profile_time::ProfilerChan,
    /// To devtools sender
    pub to_devtools_sender: Option<IpcSender<ScriptToDevtoolsControlMsg>>,
    /// From devtools sender
    pub from_devtools_sender: Option<IpcSender<DevtoolScriptControlMsg>>,
    /// Messages to send to constellation
    pub script_to_constellation_chan: ScriptToConstellationChan,
    /// Message to send to the scheduler
    pub scheduler_chan: IpcSender<TimerSchedulerMsg>,
    /// The worker id
    pub worker_id: WorkerId,
    /// The pipeline id
    pub pipeline_id: PipelineId,
    /// The origin
    pub origin: ImmutableOrigin,
    /// True if headless mode
    pub is_headless: bool,
    /// An optional string allowing the user agnet to be set for testing.
    pub user_agent: Cow<'static, str>,
}

/// Common entities representing a network load origin
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct WorkerScriptLoadOrigin {
    /// referrer url
    pub referrer_url: Option<ServoUrl>,
    /// the referrer policy which is used
    pub referrer_policy: Option<ReferrerPolicy>,
    /// the pipeline id of the entity requesting the load
    pub pipeline_id: Option<PipelineId>,
}

/// Errors from executing a paint worklet
#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum PaintWorkletError {
    /// Execution timed out.
    Timeout,
    /// No such worklet.
    WorkletNotFound,
}

impl From<RecvTimeoutError> for PaintWorkletError {
    fn from(_: RecvTimeoutError) -> PaintWorkletError {
        PaintWorkletError::Timeout
    }
}

/// Execute paint code in the worklet thread pool.
pub trait Painter: SpeculativePainter {
    /// <https://drafts.css-houdini.org/css-paint-api/#draw-a-paint-image>
    fn draw_a_paint_image(
        &self,
        size: Size2D<f32, CSSPixel>,
        zoom: Scale<f32, CSSPixel, DevicePixel>,
        properties: Vec<(Atom, String)>,
        arguments: Vec<String>,
    ) -> Result<DrawAPaintImageResult, PaintWorkletError>;
}

impl fmt::Debug for dyn Painter {
    fn fmt(&self, fmt: &mut fmt::Formatter) -> fmt::Result {
        fmt.debug_tuple("Painter")
            .field(&format_args!(".."))
            .finish()
    }
}

/// The result of executing paint code: the image together with any image URLs that need to be loaded.
///
/// TODO: this should return a WR display list. <https://github.com/servo/servo/issues/17497>
#[derive(Clone, Debug, Deserialize, MallocSizeOf, Serialize)]
pub struct DrawAPaintImageResult {
    /// The image height
    pub width: u32,
    /// The image width
    pub height: u32,
    /// The image format
    pub format: PixelFormat,
    /// The image drawn, or None if an invalid paint image was drawn
    pub image_key: Option<ImageKey>,
    /// Drawing the image might have requested loading some image URLs.
    pub missing_image_urls: Vec<ServoUrl>,
}

/// A Script to Constellation channel.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ScriptToConstellationChan {
    /// Sender for communicating with constellation thread.
    pub sender: IpcSender<(PipelineId, ScriptMsg)>,
    /// Used to identify the origin of the message.
    pub pipeline_id: PipelineId,
}

impl ScriptToConstellationChan {
    /// Send ScriptMsg and attach the pipeline_id to the message.
    pub fn send(&self, msg: ScriptMsg) -> Result<(), IpcError> {
        self.sender.send((self.pipeline_id, msg))
    }
}

/// A data-holder for serialized data and transferred objects.
/// <https://html.spec.whatwg.org/multipage/#structuredserializewithtransfer>
#[derive(Debug, Deserialize, MallocSizeOf, Serialize)]
pub struct StructuredSerializedData {
    /// Data serialized by SpiderMonkey.
    pub serialized: Vec<u8>,
    /// Transferred objects.
    pub ports: Option<HashMap<MessagePortId, MessagePortImpl>>,
}

/// A task on the https://html.spec.whatwg.org/multipage/#port-message-queue
#[derive(Debug, Deserialize, MallocSizeOf, Serialize)]
pub struct PortMessageTask {
    /// The origin of this task.
    pub origin: ImmutableOrigin,
    /// A data-holder for serialized data and transferred objects.
    pub data: StructuredSerializedData,
}

/// Messages for communication between the constellation and a global managing ports.
#[derive(Debug, Deserialize, Serialize)]
pub enum MessagePortMsg {
    /// Complete the transfer for a batch of ports.
    CompleteTransfer(HashMap<MessagePortId, VecDeque<PortMessageTask>>),
    /// Complete the transfer of a single port,
    /// whose transfer was pending because it had been requested
    /// while a previous failed transfer was being rolled-back.
    CompletePendingTransfer(MessagePortId, VecDeque<PortMessageTask>),
    /// Remove a port, the entangled one doesn't exists anymore.
    RemoveMessagePort(MessagePortId),
    /// Handle a new port-message-task.
    NewTask(MessagePortId, PortMessageTask),
}

/// The type of MediaSession action.
/// https://w3c.github.io/mediasession/#enumdef-mediasessionaction
#[derive(Clone, Debug, Deserialize, Eq, Hash, MallocSizeOf, PartialEq, Serialize)]
pub enum MediaSessionActionType {
    /// The action intent is to resume playback.
    Play,
    /// The action intent is to pause the currently active playback.
    Pause,
    /// The action intent is to move the playback time backward by a short period (i.e. a few
    /// seconds).
    SeekBackward,
    /// The action intent is to move the playback time forward by a short period (i.e. a few
    /// seconds).
    SeekForward,
    /// The action intent is to either start the current playback from the beginning if the
    /// playback has a notion, of beginning, or move to the previous item in the playlist if the
    /// playback has a notion of playlist.
    PreviousTrack,
    /// The action is to move to the playback to the next item in the playlist if the playback has
    /// a notion of playlist.
    NextTrack,
    /// The action intent is to skip the advertisement that is currently playing.
    SkipAd,
    /// The action intent is to stop the playback and clear the state if appropriate.
    Stop,
    /// The action intent is to move the playback time to a specific time.
    SeekTo,
}

impl From<i32> for MediaSessionActionType {
    fn from(value: i32) -> MediaSessionActionType {
        match value {
            1 => MediaSessionActionType::Play,
            2 => MediaSessionActionType::Pause,
            3 => MediaSessionActionType::SeekBackward,
            4 => MediaSessionActionType::SeekForward,
            5 => MediaSessionActionType::PreviousTrack,
            6 => MediaSessionActionType::NextTrack,
            7 => MediaSessionActionType::SkipAd,
            8 => MediaSessionActionType::Stop,
            9 => MediaSessionActionType::SeekTo,
            _ => panic!("Unknown MediaSessionActionType"),
        }
    }
}

/// The set of WebRender operations that can be initiated by the content process.
#[derive(Deserialize, Serialize)]
pub enum WebrenderMsg {
    /// Inform WebRender of the existence of this pipeline.
    SendInitialTransaction(DocumentId, webrender_api::PipelineId),
    /// Perform a scroll operation.
    SendScrollNode(DocumentId, LayoutPoint, ExternalScrollId, ScrollClamping),
    /// Inform WebRender of a new display list for the given pipeline.
    SendDisplayList(
        DocumentId,
        webrender_api::Epoch,
        LayoutSize,
        webrender_api::PipelineId,
        LayoutSize,
        Vec<u8>,
        BuiltDisplayListDescriptor,
    ),
    /// Perform a hit test operation. The result will be returned via
    /// the provided channel sender.
    HitTest(
        DocumentId,
        Option<webrender_api::PipelineId>,
        WorldPoint,
        HitTestFlags,
        IpcSender<HitTestResult>,
    ),
    /// Create a new image key. The result will be returned via the
    /// provided channel sender.
    GenerateImageKey(IpcSender<ImageKey>),
    /// Perform a resource update operation.
    UpdateResources(Vec<ResourceUpdate>),
}

#[derive(Clone, Deserialize, Serialize)]
/// A mechanism to communicate with the parent process' WebRender instance.
pub struct WebrenderIpcSender(IpcSender<WebrenderMsg>);

impl WebrenderIpcSender {
    /// Create a new WebrenderIpcSender object that wraps the provided channel sender.
    pub fn new(sender: IpcSender<WebrenderMsg>) -> Self {
        Self(sender)
    }

    /// Inform WebRender of the existence of this pipeline.
    pub fn send_initial_transaction(
        &self,
        document: DocumentId,
        pipeline: webrender_api::PipelineId,
    ) {
        if let Err(e) = self
            .0
            .send(WebrenderMsg::SendInitialTransaction(document, pipeline))
        {
            warn!("Error sending initial transaction: {}", e);
        }
    }

    /// Perform a scroll operation.
    pub fn send_scroll_node(
        &self,
        document: DocumentId,
        point: LayoutPoint,
        scroll_id: ExternalScrollId,
        clamping: ScrollClamping,
    ) {
        if let Err(e) = self.0.send(WebrenderMsg::SendScrollNode(
            document, point, scroll_id, clamping,
        )) {
            warn!("Error sending scroll node: {}", e);
        }
    }

    /// Inform WebRender of a new display list for the given pipeline.
    pub fn send_display_list(
        &self,
        document: DocumentId,
        epoch: Epoch,
        size: LayoutSize,
        (pipeline, size2, list): (webrender_api::PipelineId, LayoutSize, BuiltDisplayList),
    ) {
        let (data, descriptor) = list.into_data();
        if let Err(e) = self.0.send(WebrenderMsg::SendDisplayList(
            document,
            webrender_api::Epoch(epoch.0),
            size,
            pipeline,
            size2,
            data,
            descriptor,
        )) {
            warn!("Error sending display list: {}", e);
        }
    }

    /// Perform a hit test operation. Blocks until the operation is complete and
    /// and a result is available.
    pub fn hit_test(
        &self,
        document: DocumentId,
        pipeline: Option<webrender_api::PipelineId>,
        point: WorldPoint,
        flags: HitTestFlags,
    ) -> HitTestResult {
        let (sender, receiver) = ipc::channel().unwrap();
        self.0
            .send(WebrenderMsg::HitTest(
                document, pipeline, point, flags, sender,
            ))
            .expect("error sending hit test");
        receiver.recv().expect("error receiving hit test result")
    }

    /// Create a new image key. Blocks until the key is available.
    pub fn generate_image_key(&self) -> ImageKey {
        let (sender, receiver) = ipc::channel().unwrap();
        self.0
            .send(WebrenderMsg::GenerateImageKey(sender))
            .expect("error sending image key generation");
        receiver.recv().expect("error receiving image key result")
    }

    /// Perform a resource update operation.
    pub fn update_resources(&self, updates: Vec<ResourceUpdate>) {
        if let Err(e) = self.0.send(WebrenderMsg::UpdateResources(updates)) {
            warn!("error sending resource updates: {}", e);
        }
    }
}
