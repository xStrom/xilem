// Copyright 2023 the Druid Authors.
// SPDX-License-Identifier: Apache-2.0

use std::any::Any;

use xilem_core::{Id, MessageResult};

use crate::{
    context::{ChangeFlags, Cx},
    view::{DomElement, View, ViewMarker},
};

pub struct Class<V> {
    child: V,
    // This could reasonably be static Cow also, but keep things simple
    class: String,
}

pub fn class<V>(child: V, class: impl Into<String>) -> Class<V> {
    Class {
        child,
        class: class.into(),
    }
}

impl<V> ViewMarker for Class<V> {}

// TODO: make generic over A (probably requires Phantom)
impl<T, V: View<T>> View<T> for Class<V> {
    type State = V::State;
    type Element = V::Element;

    fn build(&self, cx: &mut Cx) -> (Id, Self::State, Self::Element) {
        let (id, child_state, element) = self.child.build(cx);
        element
            .as_element_ref()
            .set_attribute("class", &self.class)
            .unwrap();
        (id, child_state, element)
    }

    fn rebuild(
        &self,
        cx: &mut Cx,
        prev: &Self,
        id: &mut Id,
        state: &mut Self::State,
        element: &mut V::Element,
    ) -> ChangeFlags {
        let prev_id = *id;
        let mut changed = self.child.rebuild(cx, &prev.child, id, state, element);
        if self.class != prev.class || prev_id != *id {
            element
                .as_element_ref()
                .set_attribute("class", &self.class)
                .unwrap();
            changed.insert(ChangeFlags::OTHER_CHANGE);
        }
        changed
    }

    fn message(
        &self,
        id_path: &[Id],
        state: &mut Self::State,
        message: Box<dyn Any>,
        app_state: &mut T,
    ) -> MessageResult<()> {
        self.child.message(id_path, state, message, app_state)
    }
}
