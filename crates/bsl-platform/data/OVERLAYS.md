# Curated platform overlays

`platform_overlays.json` is project-maintained factual correction data. It is
not generated and is merged into the extracted platform JSON by `build.rs`
before Rust structures are generated. The base `platform_data.json` remains an
unchanged extract of the 1C platform help archives.

## Method parameter override schema

```json
{
  "schema_version": 1,
  "method_parameter_overrides": [
    {
      "canonical_type": "EnglishCanonicalType",
      "russian_name": "РусскоеИмяМетода",
      "english_name": "EnglishMethodName",
      "min_version": "8.1",
      "max_version": "8.3.99",
      "parameter_index": 0,
      "replacement_type_list": ["TypeA", "TypeB"],
      "evidence_source": "source identifying the platform contract",
      "rationale": "why the extracted signature needs this narrow correction"
    }
  ]
}
```

`min_version` and `max_version` are optional inclusive bounds for the target
method's documented minimum platform version. The build rejects malformed
entries, missing or ambiguous RU/EN targets, duplicate parameter overrides,
out-of-range parameter indices, invalid bounds, and duplicate type-list
members. Applying an override changes only the selected parameter type list;
method IDs and method ordering remain those of the extracted data.

## Global function parameter override schema

```json
{
  "schema_version": 1,
  "global_function_parameter_overrides": [
    {
      "russian_name": "РусскоеИмяФункции",
      "english_name": "EnglishFunctionName",
      "min_version": "8.0",
      "parameter_index": 1,
      "replacement_type_list": ["TypeA"],
      "evidence_source": "source identifying the platform contract",
      "rationale": "why the extracted signature needs this narrow correction"
    }
  ]
}
```

An entry of the form `"Семейство: *"` in `replacement_type_list` (in either
section) stands for every extracted type whose name starts with `Семейство: `,
in extract order — `"ОбъектМетаданных: *"` is each `ОбъектМетаданных: <Вид>`.
It is meant for a parameter documented as a whole family: listing some seventy
kinds by hand in each override would repeat them and drift from the extract when
it is regenerated. A wildcard that matches no extracted type is rejected.

The global-function entry is the same as a method parameter override, without `canonical_type`:
global-context functions are extracted apart from type methods and resolve by
their RU/EN names alone. The section is optional; validation and the effect of
applying an entry are those of method parameter overrides.

## Type property addition schema

```json
{
  "schema_version": 1,
  "type_property_additions": [
    {
      "canonical_type": "ClientApplicationForm",
      "russian_name": "РусскоеИмяСвойства",
      "english_name": "EnglishPropertyName",
      "property_types": ["ТипЗначения"],
      "is_readonly": false,
      "min_version": "8.2",
      "evidence_source": "source identifying the platform contract",
      "rationale": "why the extracted data omits or misfiles this property"
    }
  ]
}
```

Adds a standard property the help extract omits or files under a misleading name
— for example the managed-form `РежимОткрытияОкна`, whose help page ships under
its enum type `FormWindowOpeningMode`. `canonical_type` must be a known platform
type. Names match case-insensitively. When exactly one property of that type
carries both names, the addition is skipped and the archive record, with its
metadata and documentation, takes precedence: 8.5 archives file
`РежимОткрытияОкна` on the form itself. A property that shares only one of the
names, or several properties that each carry both, are rejected: the shared name
index would hold ambiguous entries. `is_readonly` defaults to `false` and
`min_version` is optional. When the property is added it receives a synthetic id
so documentation lookups do not alias an existing property.

## Method-local scope

Overlays are deliberately method-local. For example, a DOM `appendChild` correction
that widens an argument to accept an HTML element applies only to that method's
parameter and does not declare a global subtype relation between HTML and DOM
element types.

## Evidence requirements

Every override must include:
- `evidence_source`: A link to official documentation (ITS, syntax assistant), a minimal reproduction script proving the platform behavior, or a specific extracted platform record (for example, a syntax-assistant snippet or a JSON field reference) that demonstrates the contract.
- `rationale`: A clear explanation of why the current extracted data is insufficient and how the override improves type safety without introducing false positives.

Overrides must be justifiable by verifiable, specific evidence that supports the narrow correction without introducing false positives.
