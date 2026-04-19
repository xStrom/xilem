// Copyright 2026 the Xilem Authors
// SPDX-License-Identifier: Apache-2.0

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use masonry_testing::{ModularWidget, PRIMARY_MOUSE, ROBOTO};

use crate::app::{RenderRoot, RenderRootOptions, RenderRootSignal, WindowSizePolicy};
use crate::core::{
    Handled, LayerType, NewWidget, PointerButton, PointerButtonEvent, PointerEvent, PointerState,
    Widget, WidgetId, WindowEvent,
};
use crate::dpi::{PhysicalPosition, PhysicalSize};
use crate::kurbo::Point;
use crate::layers::Tooltip;
use crate::peniko::Blob;
use crate::theme::test_property_set;
use crate::widgets::{Label, SizedBox};

fn create_render_root(root_widget: NewWidget<impl Widget>) -> RenderRoot {
    let test_font = Blob::new(Arc::new(ROBOTO));
    let mut root = RenderRoot::new(
        root_widget,
        RenderRootOptions {
            default_properties: Arc::new(test_property_set()),
            use_system_fonts: false,
            size_policy: WindowSizePolicy::User,
            size: PhysicalSize::new(100, 100),
            scale_factor: 1.0,
            test_font: Some(test_font),
        },
    );
    root.process_signals(|_, _| {});
    root
}

#[test]
fn process_signals_drains_reentrant_host_requests() {
    let mut root = create_render_root(NewWidget::new(SizedBox::empty()));
    let new_size = PhysicalSize::new(120, 80);
    let mut seen = Vec::new();

    root.emit_signal(RenderRootSignal::SetSize(new_size));
    root.process_signals(|render_root, signal| match signal {
        RenderRootSignal::SetSize(size) => {
            seen.push("SetSize");
            assert_eq!(size, new_size);
            assert_eq!(
                render_root.handle_window_event(WindowEvent::Resize(size)),
                Handled::Yes
            );
        }
        RenderRootSignal::RequestAnimFrame => seen.push("RequestAnimFrame"),
        RenderRootSignal::RequestRedraw => seen.push("RequestRedraw"),
        other => panic!("unexpected signal while draining: {other:?}"),
    });

    assert_eq!(seen.first(), Some(&"SetSize"));
    assert!(seen.contains(&"RequestRedraw"));
    assert_eq!(root.size(), new_size);
}

#[test]
fn layer_requests_are_applied_inside_render_root() {
    let created_layer = Rc::new(Cell::new(None::<WidgetId>));
    let created_layer_id = created_layer.clone();
    let widget = ModularWidget::new(())
        .pointer_event_fn(move |_, ctx, _props, event| {
            if matches!(event, PointerEvent::Down(_)) {
                let layer = Tooltip::new(NewWidget::new(Label::new("Tooltip"))).prepare();
                created_layer_id.set(Some(layer.id()));
                ctx.create_layer(
                    LayerType::Tooltip("Tooltip".to_string()),
                    layer,
                    Point::new(5.0, 6.0),
                );
            }
        })
        .prepare();
    let mut root = create_render_root(widget);

    let mut mouse_state = PointerState::default();
    mouse_state.position = PhysicalPosition { x: 10.0, y: 10.0 };
    mouse_state.buttons.insert(PointerButton::Primary);

    let handled = root.handle_pointer_event(PointerEvent::Down(PointerButtonEvent {
        pointer: PRIMARY_MOUSE,
        button: Some(PointerButton::Primary),
        state: mouse_state,
    }));
    assert_eq!(handled, Handled::No);

    let mut host_signals = Vec::new();
    root.process_signals(|_, signal| host_signals.push(signal));

    let layer_id = created_layer
        .get()
        .expect("expected layer creation request");
    assert_eq!(root.get_layer_root(1).id(), layer_id);
    assert!(root.get_widget(layer_id).is_some());
    assert!(
        host_signals
            .iter()
            .any(|signal| matches!(signal, RenderRootSignal::RequestRedraw))
    );
}
