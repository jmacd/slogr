## Objective

A Rust logging library that:

- Compiles message formatting strings using source material
- Uses structured logging, including nested structure
- Enables formatted log recovery
- Adds a new fundamental value-reference
  - refer backwards in same log stream,
  - or refer to a qualified log record in another stream
- Includes file:line and limited number of stack frames
- Supports declaring entities that are named or anonymous
- Supports declaring attributes with descriptions and types
- Encodes definitions for callsite, stackframe, entity, attribute once
  per log stream; uses references to refer to definitions
- Reference syntax uses URN fragments mapped to JSON paths
- Encodes directly to OTLP LogRecord bytes (minimizes allocations)

## Details

### Message formats

Users of slogr can log format!-like expressions in Rust while
retaining structured formatting. Message format placeholders may
contain expression text or attribute names, and they can be
overridden. Arguments are positional.


| Statement                                | Message format              | Note                    |
|------------------------------------------|-----------------------------|-------------------------|
| `info!("hello!")`                        | "hello!"                    | no variables            |
| `info!("this {abc:s} and that {def:s}")` | "this {abc} and that {def}" | inline variable names   |
| `info!("sum {:f} quot {:f}", x+y, x/y)`  | "sum {x+y} quot {x/y}"      | `:f` => Into<f64> arg   |
| `info!("hello {}", attr = a)`            | "hello {attr}"              | `attr` must be declared |
| `info!("I am {name:s}")`                 | "I am {name}"               | `:s` => AsRef(str)      |
| `info!("see {:?}", variable)`            | "see {variable}"            | `:?` => Debug           |
| `info!("tabular {:04x} {:08x}", l, r)`   | "tabular {l:04x} {r:08x}"   | `:x` => hex             |

Generally, we aim to stay close to https://doc.rust-lang.org/std/fmt/
while adding more type format designations, generally following the C
printf letter like `:s` matches C's `%s`. The message format omits
formatting arguments that do not alter format, for example `:?` does
not explain how to format, while `:#` does.

Note that every argument must have a known type inside the macro
expansion, so that we can statically examine all logging callsites and
build known-type data structures in advance. In the future, we expect
to explore constructing Arrow record batches directly instead of OTLP
bytes, and we want to know column types in advance; however, as
discussed below, we will have to add columns to the OTAP logs
representation, and for now we are embedding our new fields within
structure of the LogRecord body field.

## Attributes

Attributes, generally those which are used by more than one callsite
are declared explicitly using OpenTelemetry Weaver semantic definition
files. In this case, the attributes specify the structured argument
type that will be encoded for the atribute, along with the attribute
name and description. A Weaver annotation supports the additional type
information which semantic conventions do not convey. When no annotation
is present, the type is presumed to be string.

```
groups:
  - id: registry.slogr_example
    type: attribute_group
    brief: Attributes used by the slogr example process.
    attributes:
      - ref: my_str   # default type is string
      - ref: my_int
        annotations:
          slogr:
            type: u64
```

Generated code allows users to import attribute definitions,

```rust
use super::attribute::{my_str, my_int};

info!("blah blah {} {}", my_str = a, my_int = b);
```

Compilation will fail if the argument cannot be interpreted as
the indicated type.

## Entities

Entities are declared similarly to attributes, however whereas
attributes define global keys, entites define value properties.
Weaver code generation will be used to declare these in a similar
manner to attributes, as they have the same representation and
semantic kind. Note that this concept of entity is not directly
related to the experimental OpenTelemetry Entities specification.

We have two categories of entity, named and anonymous. Callers
construct named entities by providing a globally distinct string-value
identifier. Semantically, a named entity is used to correlate the
entity across process boundaries; they should be used when there
is a natural given name for the entity.

Anonymous entities are a convenience for entities that are locally
unique without a natural namespace; these entities are given unique
integer identifiers, which may be randomly or sequentially assigned by
the library, and which should not be presumed equal unless assigned
within the same process. (Process identity is discussed below.)

```
groups:
  - id: registry.slogr_example
    type: attribute_group
    brief: Attributes used by the slogr example process.
    attributes:
      - ref: my_thing
        annotations:
          slogr:
            entity: named  # implies type: string
```

Callers will allocate long-lived entity-value objects:

```rust
use super::entity::{my_thing};

let handle = my_thing.new("thing_name", <expression>);

info!("starting {}", handle);
```

To avoid over-specifying this, since it requires attention to Rust
semantics and idioms, the `<expression>` above is a placeholder for a
suitable solution. The existing `as_struct()` method is suitable, as
it enables a structured-value encoding for objects. In this example,
the entity key-value `my_thing:thing_name` is defined by the expression
and logged once; future uses of the `handle` are logged as an entity
reference.

For anonymous entities, simply remove the name and let the library
fill one in:

```rust
use super::entity::{my_session};

let handle = my_session.new(<expression>);

info!("new session {}", handle);
```

Logging entity values has the effect of placing a reference to the
entity value in the log using a reference.

## Arguments

Message format strings must contain exactly as many placeholders as
there are positional arguments at the callsite. When >= 1 argument is
present, the structured values are placed under the `args` section of
the body, as a list.

```
{
  ...
  "args": [
    <first_arg>,
    <second_arg>,
    ...
  ]
}
```

## References

When an entity handle is used, it will be placed in the log record
under the `defs` section of the body. The entity handle retains a
unique identifier given to the associated log record where it was
defined in the log. Log record unique identifiers are UUID7 values
associated with a built-in entity named `log.record`.

Every log record where an entity is defined will carry a
`log.record:<uuid7>` attribute to uniquely identify itself, and entity
handles will store the record identifier where their definition
appears. The entry definition will include the entity key:value
expression and associated structured value, like `{
"my_thing:thing_name": <expression-value> }`. Note that an
`<expression-value>` may itself contain entity references.

The "on first use" criteria will require entity handles to contain
inner mutability via a `Cell` or otherwise idiomatic Rust, so as to be
able to update the log record identifier. (If a shared entity handle
is required, a `Arc<Mutex<_>>` or `OnceLock` may be required, however
this is not initially required.)

An entity reference will appear using the "$ref" key (like JSON
Schema). For example, to log a reference using an entity handle that ,
the value will appear as a reference like `{ "$ref":
"<record_uuid>#defs/my_thing:thing_name" }`, in addition to being
placed in `defs` on first use.

References are also used to convey an association between attributes
and positional arguments. For example, when a log statement uses an
`my_attr = entity_hande` argument, two associations are in effect:

1. One reference to `my_thing:thing_name` appears as a `$ref` value
   in `args[0]`, referring to the entity value, e.g., as shown above.
2. One reference appears in the record attributes, e.g. `{ "my_attr":
   { "$ref": "#args/0" } }`, referring to the same entity value.

## Additional context

https://gist.github.com/jmacd/17a6f2885d92d7864ba76680a8ee367a
