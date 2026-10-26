# Slide design and visual checks

## Existing decks and annotated templates

Inspect the actual slide size, theme and representative layouts. Identify which
objects belong to a slide, a layout and a master before changing them. A global
master change affects every linked slide, so inspect every affected slide after
it. Keep a small local edit local unless the user asks for broader changes.

Use existing placeholders and geometry when they fit the new text. Before
shrinking text, shorten the wording, move explanation to notes, or rearrange the
content. Preserve the user's requested sections and slide count. If neither is
fixed, add or remove slides only to improve coverage and pacing.

Read a reference image as evidence of hierarchy, spacing, branding and requested
changes. Do not copy annotation arrows, red boxes or placeholder instructions
into the finished slides. Preserve a logo's aspect ratio and contrast. Use a
light mark on a dark background and a suitable original variant on white when
those variants are supplied. Avoid placing a second logo over one inherited
from the layout.

## Relationships become diagrams

Choose a diagram that explains the relationship:

| Content | Useful visual | Check |
| --- | --- | --- |
| Responsibility layers | Aligned stack | Layering does not imply chronological execution. |
| Request and tool results | Directional flow or sequence | Both requests and results have an explicit direction. |
| Permission or evidence gates | Branching flow | Outcomes, including denial/failure, are visible. |
| Repeated agent execution | Feedback loop | The return path reaches the right decision. |
| Code-change workflow | Short staged flow | Each stage has one clear action or result. |
| Source selection | Decision tree | Branch labels explain when each route applies. |
| Context composition | Labeled segments | Widths are conceptual unless measured. |
| Before/after behavior | Paired examples | Both sides use the same inputs and units. |
| Measured changes over time | Native line/column chart | Values, units, periods and sources are available. |

Make diagrams editable where the target permits. Use a consistent shape meaning,
limited colors and enough empty space for labels. Connect related shapes to
anchors when the tool supports it, so later moves preserve relationships.
Verify the authoring tool's source/target arrowhead semantics in its documentation
and the rendered result. Do not assume that `head` means the target endpoint.

Prefer horizontal or vertical paths. Keep feedback paths outside body text.
Separate parallel connectors and position labels beside a line instead of on it.
Keep arrows visible at the actual presentation size. A box-and-arrow picture
adds value only when its relationships explain more than the same text in a list.

If an image is the only viable representation, keep the text editable when
possible and explain material editability limitations. Do not flatten the whole
deck into slide-sized images to conceal export problems.

## Writing and evidence

Use concrete language and one main idea per slide. Remove repeated introductions,
vague slogans and unsupported claims. Avoid placing entire paragraphs inside a
flowchart. Use parallel labels and move implementation detail into speaker notes
when the audience does not need it on screen.

Distinguish a proposed design, an illustrative example, a session-reported outcome
and a verified current result. For example, “tests passed” needs the checked
revision and scope. It does not establish production readiness. Give measured
charts their units and sources. Never turn cumulative tokens into current-context
usage or a single session's result into a general performance benchmark.

## Visual review

Start with a consistent-size overview to check flow, density and repeated
furniture. Then inspect individual slides at readable resolution. Compare edited
slides with their originals, including any master/layout content. Check long
titles, the most crowded slide, and every connector-heavy diagram carefully.

Look for text outside its box, cut-off glyphs, accidental wrapping, overlapping
labels, reversed arrowheads, unreadable footnotes, stretched images and missing
logos. Correct the underlying geometry or wording and rerender. Package checks
and a clean authoring-tool export complement visual review but cannot replace it.
