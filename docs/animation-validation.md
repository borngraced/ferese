# Animation regression checks

Run `cargo test -p ferese-animation -p ferese --offline` for transition,
resize requests, reduced motion, stacking, and refresh-rate regression tests.

For visual validation, use a disposable Ferese session with three tiled windows
containing text and at least one floating window. Test both the nested and DRM
backends. Record the session at its native refresh rate when comparing builds;
single screenshots cannot establish smoothness.

| Scenario | Expected result |
| --- | --- |
| Super+F on the middle column, then exit | Neighbours remain behind the expanding window. The shrinking window stays above them until the transition finishes. |
| Toggle Super+F repeatedly at 40–80 ms intervals | Every reversal starts at the presented frame. Corners, border and shadow follow the same zoom progress. The final state matches the last input. |
| Cycle width, then immediately Super+F | The old column-width spring cannot override fullscreen width. |
| Exit fullscreen and immediately resize or scroll | The current frame is retained when the target changes; scrolling resumes without a position jump. |
| Drag a floating window; resize from its top-left corner | The window follows the pointer directly. The opposite edge stays anchored. Delayed client commits do not move or raise it. |
| Zoom responsive and slow-redrawing clients | Each zoom requests the destination size once. Client redraws do not generate intermediate resize requests or restart the motion. |
| Zoom the leftmost and rightmost columns in and out | Each outer edge moves monotonically toward its destination; neither side trembles or reverses near the screen edge. |
| Disable borders and shadows, then zoom | Rounded corner pixels repaint through the final frame; no corner remnants remain. |
| Repeat on 60 Hz and 144 Hz outputs; move focus between them | Similar elapsed-time motion with no double advancement. Check frame timing on actual DRM hardware. |
| Enable reduced motion | Geometry and decorations reach their final values together. |

Zoom requests the destination client size once and scales the available buffer
along the transition. This avoids repeated content reflow during motion. It is
not a buffer snapshot or a guarantee of perfectly sharp intermediate text.
