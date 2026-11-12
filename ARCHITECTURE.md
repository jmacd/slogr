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
use super::attributes::my_attr;

info!("blah blah {} {}", my_str = a, my_int = b); 
```

Compilation will fail if the argument cannot be converted to
the indicated type.

## Entities

Entities are declared similarly to attributes, however whereas
attributes define keys, entites define values.

## References

https://gist.github.com/jmacd/17a6f2885d92d7864ba76680a8ee367a

