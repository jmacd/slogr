// Copyright (c) 2019 Tokio Contributors
// SPDX-License-Identifier: MIT
// Adapted from tracing 0.1.44; see NOTICE and LICENSE-TRACING.

//! Locally owned event macro machinery. Runtime types remain tracing's.
//! Only named, explicitly targeted events used by the public macros are retained.

use tracing::callsite::{DefaultCallsite, Identifier};
use tracing::field::{Field, FieldSet};
use tracing::metadata::Kind;
use tracing::subscriber::Interest;
use tracing::{Level, Metadata};

#[doc(hidden)]
#[macro_export]
macro_rules! __slogr_emit {
    // Name / target / parent.
    (level: $lvl:expr, name: $name:expr, target: $target:expr, parent: $parent:expr, { $($field:tt)* }, $($arg:tt)* ) => (
        $crate::__slogr_event!(name: $name, target: $target, parent: $parent, $lvl, { $($field)* }, $($arg)*)
    );
    (level: $lvl:expr, name: $name:expr, target: $target:expr, parent: $parent:expr, $($k:ident).+ $($field:tt)* ) => (
        $crate::__slogr_event!(name: $name, target: $target, parent: $parent, $lvl, { $($k).+ $($field)* })
    );
    (level: $lvl:expr, name: $name:expr, target: $target:expr, parent: $parent:expr, ?$($k:ident).+ $($field:tt)* ) => (
        $crate::__slogr_event!(name: $name, target: $target, parent: $parent, $lvl, { ?$($k).+ $($field)* })
    );
    (level: $lvl:expr, name: $name:expr, target: $target:expr, parent: $parent:expr, %$($k:ident).+ $($field:tt)* ) => (
        $crate::__slogr_event!(name: $name, target: $target, parent: $parent, $lvl, { %$($k).+ $($field)* })
    );
    (level: $lvl:expr, name: $name:expr, target: $target:expr, parent: $parent:expr, $($arg:tt)+ ) => (
        $crate::__slogr_event!(name: $name, target: $target, parent: $parent, $lvl, {}, $($arg)+)
    );

    // Name / target.
    (level: $lvl:expr, name: $name:expr, target: $target:expr, { $($field:tt)* }, $($arg:tt)* ) => (
        $crate::__slogr_event!(name: $name, target: $target, $lvl, { $($field)* }, $($arg)*)
    );
    (level: $lvl:expr, name: $name:expr, target: $target:expr, $($k:ident).+ $($field:tt)* ) => (
        $crate::__slogr_event!(name: $name, target: $target, $lvl, { $($k).+ $($field)* })
    );
    (level: $lvl:expr, name: $name:expr, target: $target:expr, ?$($k:ident).+ $($field:tt)* ) => (
        $crate::__slogr_event!(name: $name, target: $target, $lvl, { ?$($k).+ $($field)* })
    );
    (level: $lvl:expr, name: $name:expr, target: $target:expr, %$($k:ident).+ $($field:tt)* ) => (
        $crate::__slogr_event!(name: $name, target: $target, $lvl, { %$($k).+ $($field)* })
    );
    (level: $lvl:expr, name: $name:expr, target: $target:expr, $($arg:tt)+ ) => (
        $crate::__slogr_event!(name: $name, target: $target, $lvl, {}, $($arg)+)
    );

}

#[doc(hidden)]
#[macro_export]
macro_rules! __slogr_event {
    // Name / target / parent.
    (name: $name:expr, target: $target:expr, parent: $parent:expr, $lvl:expr, { $($fields:tt)* } )=> ({
        use $crate::_private::Callsite as _;
        static __CALLSITE: $crate::_private::DefaultCallsite = $crate::__slogr_callsite! {
            name: $name,
            kind: $crate::_private::Kind::EVENT,
            target: $target,
            level: $lvl,
            fields: $($fields)*
        };

        let enabled = $crate::__slogr_level_enabled!($lvl) && {
            let interest = __CALLSITE.interest();
            !interest.is_never() && $crate::_private::is_enabled(__CALLSITE.metadata(), interest)
        };
        if enabled {
            $crate::__slogr_valueset_all!(__CALLSITE.metadata().fields(), |value_set: $crate::_private::field::ValueSet| {
                let meta = __CALLSITE.metadata();
                // event with explicit parent
                $crate::_private::Event::child_of(
                    $parent,
                    meta,
                    &value_set
                );
            }; $($fields)*);
        }
    });
    (name: $name:expr, target: $target:expr, parent: $parent:expr, $lvl:expr, { $($fields:tt)* }, $($arg:tt)+ ) => (
        $crate::__slogr_event!(
            name: $name,
            target: $target,
            parent: $parent,
            $lvl,
            { message = $crate::_private::format_args!($($arg)+), $($fields)* }
        )
    );
    (name: $name:expr, target: $target:expr, parent: $parent:expr, $lvl:expr, $($k:ident).+ = $($fields:tt)* ) => (
        $crate::__slogr_event!(name: $name, target: $target, parent: $parent, $lvl, { $($k).+ = $($fields)* })
    );
    (name: $name:expr, target: $target:expr, parent: $parent:expr, $lvl:expr, $($arg:tt)+) => (
        $crate::__slogr_event!(name: $name, target: $target, parent: $parent, $lvl, { $($arg)+ })
    );

    // Name / target.
    (name: $name:expr, target: $target:expr, $lvl:expr, { $($fields:tt)* } )=> ({
        use $crate::_private::Callsite as _;
        static __CALLSITE: $crate::_private::DefaultCallsite = $crate::__slogr_callsite! {
            name: $name,
            kind: $crate::_private::Kind::EVENT,
            target: $target,
            level: $lvl,
            fields: $($fields)*
        };
        let enabled = $crate::__slogr_level_enabled!($lvl) && {
            let interest = __CALLSITE.interest();
            !interest.is_never() && $crate::_private::is_enabled(__CALLSITE.metadata(), interest)
        };
        if enabled {
            $crate::__slogr_valueset_all!(__CALLSITE.metadata().fields(), |value_set: $crate::_private::field::ValueSet| {
                let meta = __CALLSITE.metadata();
                // event with contextual parent
                $crate::_private::Event::dispatch(
                    meta,
                    &value_set
                );
            }; $($fields)*);
        }
    });
    (name: $name:expr, target: $target:expr, $lvl:expr, { $($fields:tt)* }, $($arg:tt)+ ) => (
        $crate::__slogr_event!(
            name: $name,
            target: $target,
            $lvl,
            { message = $crate::_private::format_args!($($arg)+), $($fields)* }
        )
    );
    (name: $name:expr, target: $target:expr, $lvl:expr, $($k:ident).+ = $($fields:tt)* ) => (
        $crate::__slogr_event!(name: $name, target: $target, $lvl, { $($k).+ = $($fields)* })
    );
    (name: $name:expr, target: $target:expr, $lvl:expr, $($arg:tt)+) => (
        $crate::__slogr_event!(name: $name, target: $target, $lvl, { $($arg)+ })
    );

}

#[doc(hidden)]
#[macro_export]
macro_rules! __slogr_callsite {
    (
        name: $name:expr,
        kind: $kind:expr,
        target: $target:expr,
        level: $lvl:expr,
        fields: $($fields:tt)*
    ) => {{
        static META: $crate::_private::Metadata<'static> = $crate::_private::Metadata::new(
            $name,
            $target,
            $lvl,
            $crate::_private::Option::Some($crate::_private::file!()),
            $crate::_private::Option::Some($crate::_private::line!()),
            $crate::_private::Option::Some($crate::_private::module_path!()),
            $crate::_private::field::FieldSet::new(
                $crate::__slogr_fieldset!($($fields)*),
                $crate::_private::Identifier(&__CALLSITE),
            ),
            $kind,
        );
        $crate::_private::DefaultCallsite::new(&META)
    }};
}

#[doc(hidden)]
#[macro_export]
macro_rules! __slogr_level_enabled {
    ($lvl:expr) => {
        $lvl <= $crate::_private::level_filters::STATIC_MAX_LEVEL
            && $lvl <= $crate::_private::level_filters::LevelFilter::current()
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __slogr_valueset_all {

    // === base case ===
    (@ { $(,)* $($val:expr),* $(,)* } $(,)*) => {
        &[ $($val),* ]
    };

    // === recursive case (more tts) ===

    (@ { $(,)* $($out:expr),* }, $($k:ident).+ = ?$val:expr, $($rest:tt)*) => {
        $crate::__slogr_valueset_all!(
            @ { $($out),*, ($crate::_private::Option::Some(&$crate::_private::field::debug(&$val) as &dyn $crate::_private::field::Value)) },
            $($rest)*
        )
    };
    (@ { $(,)* $($out:expr),* }, $($k:ident).+ = %$val:expr, $($rest:tt)*) => {
        $crate::__slogr_valueset_all!(
            @ { $($out),*, ($crate::_private::Option::Some(&$crate::_private::field::display(&$val) as &dyn $crate::_private::field::Value)) },
            $($rest)*
        )
    };
    (@ { $(,)* $($out:expr),* }, $($k:ident).+ = $val:expr, $($rest:tt)*) => {
        $crate::__slogr_valueset_all!(
            @ { $($out),*, ($crate::_private::Option::Some(&$val as &dyn $crate::_private::field::Value)) },
            $($rest)*
        )
    };
    (@ { $(,)* $($out:expr),* }, $($k:ident).+, $($rest:tt)*) => {
        $crate::__slogr_valueset_all!(
            @ { $($out),*, ($crate::_private::Option::Some(&$($k).+ as &dyn $crate::_private::field::Value)) },
            $($rest)*
        )
    };
    (@ { $(,)* $($out:expr),* }, ?$($k:ident).+, $($rest:tt)*) => {
        $crate::__slogr_valueset_all!(
            @ { $($out),*, ($crate::_private::Option::Some(&$crate::_private::field::debug(&$($k).+) as &dyn $crate::_private::field::Value)) },
            $($rest)*
        )
    };
    (@ { $(,)* $($out:expr),* }, %$($k:ident).+, $($rest:tt)*) => {
        $crate::__slogr_valueset_all!(
            @ { $($out),*, ($crate::_private::Option::Some(&$crate::_private::field::display(&$($k).+) as &dyn $crate::_private::field::Value)) },
            $($rest)*
        )
    };
    (@ { $(,)* $($out:expr),* }, $($k:ident).+ = ?$val:expr) => {
        $crate::__slogr_valueset_all!(
            @ { $($out),*, ($crate::_private::Option::Some(&$crate::_private::field::debug(&$val) as &dyn $crate::_private::field::Value)) },
        )
    };
    (@ { $(,)* $($out:expr),* }, $($k:ident).+ = %$val:expr) => {
        $crate::__slogr_valueset_all!(
            @ { $($out),*, ($crate::_private::Option::Some(&$crate::_private::field::display(&$val) as &dyn $crate::_private::field::Value)) },
        )
    };
    (@ { $(,)* $($out:expr),* }, $($k:ident).+ = $val:expr) => {
        $crate::__slogr_valueset_all!(
            @ { $($out),*, ($crate::_private::Option::Some(&$val as &dyn $crate::_private::field::Value)) },
        )
    };
    (@ { $(,)* $($out:expr),* }, $($k:ident).+) => {
        $crate::__slogr_valueset_all!(
            @ { $($out),*, ($crate::_private::Option::Some(&$($k).+ as &dyn $crate::_private::field::Value)) },
        )
    };
    (@ { $(,)* $($out:expr),* }, ?$($k:ident).+) => {
        $crate::__slogr_valueset_all!(
            @ { $($out),*, ($crate::_private::Option::Some(&$crate::_private::field::debug(&$($k).+) as &dyn $crate::_private::field::Value)) },
        )
    };
    (@ { $(,)* $($out:expr),* }, %$($k:ident).+) => {
        $crate::__slogr_valueset_all!(
            @ { $($out),*, ($crate::_private::Option::Some(&$crate::_private::field::display(&$($k).+) as &dyn $crate::_private::field::Value)) },
        )
    };

    // Handle literal names
    (@ { $(,)* $($out:expr),* }, $k:literal = ?$val:expr, $($rest:tt)*) => {
        $crate::__slogr_valueset_all!(
            @ { $($out),*, ($crate::_private::Option::Some(&$crate::_private::field::debug(&$val) as &dyn $crate::_private::field::Value)) },
            $($rest)*
        )
    };
    (@ { $(,)* $($out:expr),* }, $k:literal = %$val:expr, $($rest:tt)*) => {
        $crate::__slogr_valueset_all!(
            @ { $($out),*, ($crate::_private::Option::Some(&$crate::_private::field::display(&$val) as &dyn $crate::_private::field::Value)) },
            $($rest)*
        )
    };
    (@ { $(,)* $($out:expr),* }, $k:literal = $val:expr, $($rest:tt)*) => {
        $crate::__slogr_valueset_all!(
            @ { $($out),*, ($crate::_private::Option::Some(&$val as &dyn $crate::_private::field::Value)) },
            $($rest)*
        )
    };
    (@ { $(,)* $($out:expr),* }, $k:literal = ?$val:expr) => {
        $crate::__slogr_valueset_all!(
            @ { $($out),*, ($crate::_private::Option::Some(&$crate::_private::field::debug(&$val) as &dyn $crate::_private::field::Value)) },
        )
    };
    (@ { $(,)* $($out:expr),* }, $k:literal = %$val:expr) => {
        $crate::__slogr_valueset_all!(
            @ { $($out),*, ($crate::_private::Option::Some(&$crate::_private::field::display(&$val) as &dyn $crate::_private::field::Value)) },
        )
    };
    (@ { $(,)* $($out:expr),* }, $k:literal = $val:expr) => {
        $crate::__slogr_valueset_all!(
            @ { $($out),*, ($crate::_private::Option::Some(&$val as &dyn $crate::_private::field::Value)) },
        )
    };

    // Handle constant names
    (@ { $(,)* $($out:expr),* }, { $k:expr } = ?$val:expr, $($rest:tt)*) => {
        $crate::__slogr_valueset_all!(
            @ { $($out),*, (Some(&$crate::_private::field::debug(&$val) as &dyn $crate::_private::field::Value)) },
            $($rest)*
        )
    };
    (@ { $(,)* $($out:expr),* }, { $k:expr } = %$val:expr, $($rest:tt)*) => {
        $crate::__slogr_valueset_all!(
            @ { $($out),*, (Some(&$crate::_private::field::display(&$val) as &dyn $crate::_private::field::Value)) },
            $($rest)*
        )
    };
    (@ { $(,)* $($out:expr),* }, { $k:expr } = $val:expr, $($rest:tt)*) => {
        $crate::__slogr_valueset_all!(
            @ { $($out),*, (Some(&$val as &dyn $crate::_private::field::Value)) },
            $($rest)*
        )
    };
    (@ { $(,)* $($out:expr),* }, { $k:expr } = ?$val:expr) => {
        $crate::__slogr_valueset_all!(
            @ { $($out),*, (Some(&$crate::_private::field::debug(&$val) as &dyn $crate::_private::field::Value)) },
        )
    };
    (@ { $(,)* $($out:expr),* }, { $k:expr } = %$val:expr) => {
        $crate::__slogr_valueset_all!(
            @ { $($out),*, (Some(&$crate::_private::field::display(&$val) as &dyn $crate::_private::field::Value)) },
        )
    };
    (@ { $(,)* $($out:expr),* }, { $k:expr } = $val:expr) => {
        $crate::__slogr_valueset_all!(
            @ { $($out),*, (Some(&$val as &dyn $crate::_private::field::Value)) },
        )
    };

    // Remainder is unparsable, but exists --- must be format args!
    (@ { $(,)* $($out:expr),* }, $($rest:tt)+) => {
        $crate::__slogr_valueset_all!(@ { ($crate::_private::Option::Some(&$crate::_private::format_args!($($rest)+) as &dyn $crate::_private::field::Value)), $($out),* },)
    };

    // === entry ===
    ($fields:expr, $consume:expr; $($kvs:tt)+) => {
        {
            #[allow(unused_imports)]
            // This import statement CANNOT be removed as it will break existing use cases.
            // See #831, #2332, #3424 for the last times we tried.
            use $crate::_private::field::{debug, display, Value};
            // Rust 2024 drops tail-expression temporaries at the block boundary.
            // Consume the borrowed values here instead of returning a ValueSet.
            ($consume)($fields.value_set_all($crate::__slogr_valueset_all!(
                @ { },
                $($kvs)+
            )))
        }
    };
    ($fields:expr, $consume:expr;) => {
        {
            ($consume)($fields.value_set_all(&[]))
        }
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __slogr_valueset {

    // === base case ===
    (@ $fields:expr, { $(,)* $(($field:expr, $val:expr)),* $(,)* } $(,)*) => {
        &[
            $((
                $fields.field($field).as_ref().unwrap_or(&$crate::_private::FAKE_FIELD),
                $crate::_private::Option::Some(&$val as &dyn $crate::_private::field::Value),
            ),)*
        ]
    };

    // === recursive case (more tts) ===

    (@ $fields:expr, { $(,)* $(($field:expr, $out:expr)),* }, $($k:ident).+ = ?$val:expr, $($rest:tt)*) => {
        $crate::__slogr_valueset!(
            @ $fields, { $(($field, $out)),*, ($crate::__slogr_stringify!($($k).+), $crate::_private::field::debug(&$val)) },
            $($rest)*
        )
    };
    (@ $fields:expr, { $(,)* $(($field:expr, $out:expr)),* }, $($k:ident).+ = %$val:expr, $($rest:tt)*) => {
        $crate::__slogr_valueset!(
            @ $fields, { $(($field, $out)),*, ($crate::__slogr_stringify!($($k).+), $crate::_private::field::display(&$val)) },
            $($rest)*
        )
    };
    (@ $fields:expr, { $(,)* $(($field:expr, $out:expr)),* }, $($k:ident).+ = $val:expr, $($rest:tt)*) => {
        $crate::__slogr_valueset!(
            @ $fields, { $(($field, $out)),*, ($crate::__slogr_stringify!($($k).+), $val) },
            $($rest)*
        )
    };
    (@ $fields:expr, { $(,)* $(($field:expr, $out:expr)),* }, $($k:ident).+, $($rest:tt)*) => {
        $crate::__slogr_valueset!(
            @ $fields, { $(($field, $out)),*, ($crate::__slogr_stringify!($($k).+), $($k).+) },
            $($rest)*
        )
    };
    (@ $fields:expr, { $(,)* $(($field:expr, $out:expr)),* }, ?$($k:ident).+, $($rest:tt)*) => {
        $crate::__slogr_valueset!(
            @ $fields, { $(($field, $out)),*, ($crate::__slogr_stringify!($($k).+), $crate::_private::field::debug(&$($k).+)) },
            $($rest)*
        )
    };
    (@ $fields:expr, { $(,)* $(($field:expr, $out:expr)),* }, %$($k:ident).+, $($rest:tt)*) => {
        $crate::__slogr_valueset!(
            @ $fields, { $(($field, $out)),*, ($crate::__slogr_stringify!($($k).+), $crate::_private::field::display(&$($k).+)) },
            $($rest)*
        )
    };
    (@ $fields:expr, { $(,)* $(($field:expr, $out:expr)),* }, $($k:ident).+ = ?$val:expr) => {
        $crate::__slogr_valueset!(
            @ $fields, { $(($field, $out)),*, ($crate::__slogr_stringify!($($k).+), $crate::_private::field::debug(&$val)) },
        )
    };
    (@ $fields:expr, { $(,)* $(($field:expr, $out:expr)),* }, $($k:ident).+ = %$val:expr) => {
        $crate::__slogr_valueset!(
            @ $fields, { $(($field, $out)),*, ($crate::__slogr_stringify!($($k).+), $crate::_private::field::display(&$val)) },
        )
    };
    (@ $fields:expr, { $(,)* $(($field:expr, $out:expr)),* }, $($k:ident).+ = $val:expr) => {
        $crate::__slogr_valueset!(
            @ $fields, { $(($field, $out)),*, ($crate::__slogr_stringify!($($k).+), $val) },
        )
    };
    (@ $fields:expr, { $(,)* $(($field:expr, $out:expr)),* }, $($k:ident).+) => {
        $crate::__slogr_valueset!(
            @ $fields, { $(($field, $out)),*, ($crate::__slogr_stringify!($($k).+), $($k).+) },
        )
    };
    (@ $fields:expr, { $(,)* $(($field:expr, $out:expr)),* }, ?$($k:ident).+) => {
        $crate::__slogr_valueset!(
            @ $fields, { $(($field, $out)),*, ($crate::__slogr_stringify!($($k).+), $crate::_private::field::debug(&$($k).+)) },
        )
    };
    (@ $fields:expr, { $(,)* $(($field:expr, $out:expr)),* }, %$($k:ident).+) => {
        $crate::__slogr_valueset!(
            @ $fields, { $(($field, $out)),*, ($crate::__slogr_stringify!($($k).+), $crate::_private::field::display(&$($k).+)) },
        )
    };

    // Handle literal names
    (@ $fields:expr, { $(,)* $(($field:expr, $out:expr)),* }, $k:literal = ?$val:expr, $($rest:tt)*) => {
        $crate::__slogr_valueset!(
            @ $fields, { $(($field, $out)),*, ($k, $crate::_private::field::debug(&$val)) },
            $($rest)*
        )
    };
    (@ $fields:expr, { $(,)* $(($field:expr, $out:expr)),* }, $k:literal = %$val:expr, $($rest:tt)*) => {
        $crate::__slogr_valueset!(
            @ $fields, { $(($field, $out)),*, ($k, $crate::_private::field::display(&$val)) },
            $($rest)*
        )
    };
    (@ $fields:expr, { $(,)* $(($field:expr, $out:expr)),* }, $k:literal = $val:expr, $($rest:tt)*) => {
        $crate::__slogr_valueset!(
            @ $fields, { $(($field, $out)),*, ($k, $val) },
            $($rest)*
        )
    };
    (@ $fields:expr, { $(,)* $(($field:expr, $out:expr)),* }, $k:literal = ?$val:expr) => {
        $crate::__slogr_valueset!(
            @ $fields, { $(($field, $out)),*, ($k, $crate::_private::field::debug(&$val)) },
        )
    };
    (@ $fields:expr, { $(,)* $(($field:expr, $out:expr)),* }, $k:literal = %$val:expr) => {
        $crate::__slogr_valueset!(
            @ $fields, { $(($field, $out)),*, ($k, $crate::_private::field::display(&$val)) },
        )
    };
    (@ $fields:expr, { $(,)* $(($field:expr, $out:expr)),* }, $k:literal = $val:expr) => {
        $crate::__slogr_valueset!(
            @ $fields, { $(($field, $out)),*, ($k, $val) },
        )
    };

    // Handle constant names
    (@ $fields:expr, { $(,)* $(($field:expr, $out:expr)),* }, { $k:expr } = ?$val:expr, $($rest:tt)*) => {
        $crate::__slogr_valueset!(
            @ $fields, { $(($field, $out)),*, ($k, $crate::_private::field::debug(&$val)) },
            $($rest)*
        )
    };
    (@ $fields:expr, { $(,)* $(($field:expr, $out:expr)),* }, { $k:expr } = %$val:expr, $($rest:tt)*) => {
        $crate::__slogr_valueset!(
            @ $fields, { $(($field, $out)),*, ($k, $crate::_private::field::display(&$val)) },
            $($rest)*
        )
    };
    (@ $fields:expr, { $(,)* $(($field:expr, $out:expr)),* }, { $k:expr } = $val:expr, $($rest:tt)*) => {
        $crate::__slogr_valueset!(
            @ $fields, { $(($field, $out)),*, ($k, $val) },
            $($rest)*
        )
    };
    (@ $fields:expr, { $(,)* $(($field:expr, $out:expr)),* }, { $k:expr } = ?$val:expr) => {
        $crate::__slogr_valueset!(
            @ $fields, { $(($field, $out)),*, ($k, $crate::_private::field::debug(&$val)) },
        )
    };
    (@ $fields:expr, { $(,)* $(($field:expr, $out:expr)),* }, { $k:expr } = %$val:expr) => {
        $crate::__slogr_valueset!(
            @ $fields, { $(($field, $out)),*, ($k, $crate::_private::field::display(&$val)) },
        )
    };
    (@ $fields:expr, { $(,)* $(($field:expr, $out:expr)),* }, { $k:expr } = $val:expr) => {
        $crate::__slogr_valueset!(
            @ $fields, { $(($field, $out)),*, ($k, $val) },
        )
    };

    // === entry ===
    ($fields:expr, $consume:expr; $($kvs:tt)+) => {
        {
            #[allow(unused_imports)]
            // This import statement CANNOT be removed as it will break existing use cases.
            // See #831, #2332, #3424 for the last times we tried.
            use $crate::_private::field::{debug, display, Value};
            ($consume)($fields.value_set($crate::__slogr_valueset!(
                @ $fields, { },
                $($kvs)+
            )))
        }
    };
    ($fields:expr, $consume:expr;) => {
        {
            ($consume)($fields.value_set(&[]))
        }
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __slogr_fieldset {
    // == base case ==
    (@ { $(,)* $($out:expr),* $(,)* } $(,)*) => {
        &[ $($out),* ]
    };

    // == recursive cases (more tts) ==
    (@ { $(,)* $($out:expr),* } $($k:ident).+ = ?$val:expr, $($rest:tt)*) => {
        $crate::__slogr_fieldset!(@ { $($out),*, $crate::__slogr_stringify!($($k).+) } $($rest)*)
    };
    (@ { $(,)* $($out:expr),* } $($k:ident).+ = %$val:expr, $($rest:tt)*) => {
        $crate::__slogr_fieldset!(@ { $($out),*, $crate::__slogr_stringify!($($k).+) } $($rest)*)
    };
    (@ { $(,)* $($out:expr),* } $($k:ident).+ = $val:expr, $($rest:tt)*) => {
        $crate::__slogr_fieldset!(@ { $($out),*, $crate::__slogr_stringify!($($k).+) } $($rest)*)
    };
    (@ { $(,)* $($out:expr),* } ?$($k:ident).+, $($rest:tt)*) => {
        $crate::__slogr_fieldset!(@ { $($out),*, $crate::__slogr_stringify!($($k).+) } $($rest)*)
    };
    (@ { $(,)* $($out:expr),* } %$($k:ident).+, $($rest:tt)*) => {
        $crate::__slogr_fieldset!(@ { $($out),*, $crate::__slogr_stringify!($($k).+) } $($rest)*)
    };
    (@ { $(,)* $($out:expr),* } $($k:ident).+, $($rest:tt)*) => {
        $crate::__slogr_fieldset!(@ { $($out),*, $crate::__slogr_stringify!($($k).+) } $($rest)*)
    };

    // Handle literal names
    (@ { $(,)* $($out:expr),* } $k:literal = ?$val:expr, $($rest:tt)*) => {
        $crate::__slogr_fieldset!(@ { $($out),*, $k } $($rest)*)
    };
    (@ { $(,)* $($out:expr),* } $k:literal = %$val:expr, $($rest:tt)*) => {
        $crate::__slogr_fieldset!(@ { $($out),*, $k } $($rest)*)
    };
    (@ { $(,)* $($out:expr),* } $k:literal = $val:expr, $($rest:tt)*) => {
        $crate::__slogr_fieldset!(@ { $($out),*, $k } $($rest)*)
    };

    // Handle constant names
    (@ { $(,)* $($out:expr),* } { $k:expr } = ?$val:expr, $($rest:tt)*) => {
        $crate::__slogr_fieldset!(@ { $($out),*, $k } $($rest)*)
    };
    (@ { $(,)* $($out:expr),* } { $k:expr } = %$val:expr, $($rest:tt)*) => {
        $crate::__slogr_fieldset!(@ { $($out),*, $k } $($rest)*)
    };
    (@ { $(,)* $($out:expr),* } { $k:expr } = $val:expr, $($rest:tt)*) => {
        $crate::__slogr_fieldset!(@ { $($out),*, $k } $($rest)*)
    };

    // Remainder is unparsable, but exists --- must be format args!
    (@ { $(,)* $($out:expr),* } $($rest:tt)+) => {
        $crate::__slogr_fieldset!(@ { "message", $($out),*, })
    };

    // == entry ==
    ($($args:tt)*) => {
        $crate::__slogr_fieldset!(@ { } $($args)*,)
    };

}

#[doc(hidden)]
#[macro_export]
macro_rules! __slogr_stringify {
    ($($k:ident).+) => {{
        const NAME: $crate::_private::FieldName<{
            $crate::_private::FieldName::len($crate::_private::stringify!($($k).+))
        }> = $crate::_private::FieldName::new($crate::_private::stringify!($($k).+));
        NAME.as_str()
    }};
}

/// Implementation detail used for constructing FieldSet names from raw
/// identifiers. In `info!(..., r#type = "...")` the macro would end up
/// constructing a name equivalent to `FieldName(*b"type")`.
pub struct FieldName<const N: usize>([u8; N]);

impl<const N: usize> FieldName<N> {
    /// Convert `"prefix.r#keyword.suffix"` to `b"prefix.keyword.suffix"`.
    pub const fn new(input: &str) -> Self {
        let input = input.as_bytes();
        let mut output = [0u8; N];
        let mut read = 0;
        let mut write = 0;
        while read < input.len() {
            if read + 1 < input.len() && input[read] == b'r' && input[read + 1] == b'#' {
                read += 2;
            }
            output[write] = input[read];
            read += 1;
            write += 1;
        }
        assert!(write == N);
        Self(output)
    }

    pub const fn as_str(&self) -> &str {
        // Unlike the upstream helper, use safe const UTF-8 validation.
        match std::str::from_utf8(self.0.as_slice()) {
            Ok(name) => name,
            Err(_) => panic!("field name must be UTF-8"),
        }
    }
}

impl FieldName<0> {
    /// For `"prefix.r#keyword.suffix"` compute `"prefix.keyword.suffix".len()`.
    pub const fn len(input: &str) -> usize {
        // Count occurrences of "r#"
        let mut raw = 0;

        let mut i = 0;
        while i < input.len() {
            if input.as_bytes()[i] == b'#' {
                raw += 1;
            }
            i += 1;
        }

        input.len() - 2 * raw
    }
}

pub fn is_enabled(meta: &Metadata<'static>, interest: Interest) -> bool {
    interest.is_always() || tracing::dispatcher::get_default(|default| default.enabled(meta))
}

static CALLSITE: DefaultCallsite = DefaultCallsite::new(&META);
static META: Metadata<'static> = Metadata::new(
    "__fake_slogr_callsite",
    module_path!(),
    Level::TRACE,
    Some(file!()),
    Some(line!()),
    Some(module_path!()),
    FieldSet::new(&[], Identifier(&CALLSITE)),
    Kind::SPAN,
);
pub static FAKE_FIELD: Field = META.private_fake_field();
