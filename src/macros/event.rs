// Copyright (c) 2019 Tokio Contributors
// SPDX-License-Identifier: MIT
// Adapted from tracing 0.1.44; see NOTICE and LICENSE-TRACING.

//! Locally owned callsites and dispatch for body-first, typed logging.

use tracing::Metadata;
use tracing::subscriber::Interest;

// Marks the mandatory first field, without giving any user attribute a special name.
pub const BODY_FIELD_NAME: &str = "__slogr_body";

#[inline]
pub fn body(value: &str) -> &str {
    value
}

pub fn is_enabled(meta: &Metadata<'static>, interest: Interest) -> bool {
    interest.is_always() || tracing::dispatcher::get_default(|default| default.enabled(meta))
}

#[doc(hidden)]
#[macro_export]
macro_rules! __slogr_callsite {
    ($level:expr; $($attribute:path),*) => {{
        static META: $crate::_private::Metadata<'static> = $crate::_private::Metadata::new(
            $crate::_private::concat!(
                "event ", $crate::_private::file!(), ":", $crate::_private::line!()
            ),
            env!("CARGO_PKG_NAME"),
            $level,
            $crate::_private::Option::Some($crate::_private::file!()),
            $crate::_private::Option::Some($crate::_private::line!()),
            $crate::_private::Option::Some($crate::_private::module_path!()),
            $crate::_private::field::FieldSet::new(
                &[
                    $crate::_private::BODY_FIELD_NAME,
                    $($crate::Attribute::name(&$attribute)),*
                ],
                $crate::_private::Identifier(&__CALLSITE),
            ),
            $crate::_private::Kind::EVENT,
        );
        $crate::_private::DefaultCallsite::new(&META)
    }};
}

#[doc(hidden)]
#[macro_export]
macro_rules! __slogr_level_enabled {
    ($level:expr) => {
        $level <= $crate::_private::level_filters::STATIC_MAX_LEVEL
            && $level <= $crate::_private::level_filters::LevelFilter::current()
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __slogr_values {
    ($fields:expr, $consume:expr; $body:expr; $($attribute:path = $value:expr),*) => {{
        // Consume borrows in their construction scope, including Rust 2024 temporaries.
        // The value expressions remain outside the callback so `?` and nested logs work.
        ($consume)($fields.value_set_all(&[
            $crate::_private::Option::Some(
                &$crate::_private::body(&$body) as &dyn $crate::_private::field::Value
            ),
            $(
                $crate::_private::Option::Some(
                    &$crate::Attribute::__value(&$attribute, &$value)
                        as &dyn $crate::_private::field::Value
                )
            ),*
        ]))
    }};
}

#[doc(hidden)]
#[macro_export]
macro_rules! __slogr_log {
    ($level:expr, $body:expr; logger: $logger:expr $(, $attribute:path = $value:expr)* $(,)?) => {{
        use $crate::_private::Callsite as _;
        if $crate::__slogr_level_enabled!($level) {
            static __CALLSITE: $crate::_private::DefaultCallsite =
                $crate::__slogr_callsite!($level; $($attribute),*);
            let interest = __CALLSITE.interest();
            let metadata = __CALLSITE.metadata();
            if !interest.is_never() && $crate::_private::is_enabled(metadata, interest) {
                let logger = &mut ($logger);
                if $crate::Sampler::should_sample(logger, metadata) {
                    $crate::__slogr_values!(metadata.fields(), |values: $crate::_private::ValueSet<'_>| {
                        let event = $crate::_private::Event::new(metadata, &values);
                        $crate::_private::get_default(|dispatch| {
                            $crate::Logger::emit(logger, &event, dispatch);
                        });
                    }; $body; $($attribute = $value),*);
                }
            }
        }
    }};
    ($level:expr, $body:expr; $($attribute:path = $value:expr),* $(,)?) => {{
        use $crate::_private::Callsite as _;
        if $crate::__slogr_level_enabled!($level) {
            static __CALLSITE: $crate::_private::DefaultCallsite =
                $crate::__slogr_callsite!($level; $($attribute),*);
            let interest = __CALLSITE.interest();
            let metadata = __CALLSITE.metadata();
            if !interest.is_never() && $crate::_private::is_enabled(metadata, interest) {
                $crate::__slogr_values!(metadata.fields(), |values: $crate::_private::ValueSet<'_>| {
                    $crate::_private::Event::dispatch(metadata, &values);
                }; $body; $($attribute = $value),*);
            }
        }
    }};
    ($level:expr, $body:expr; $($invalid:tt)+) => {
        compile_error!("expected ATTRIBUTE = value; free-form fields and formatting overrides are not supported")
    };
}
