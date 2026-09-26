# Comic features (observation-based specification)

This specification describes the manga/comic features Efude implements. It is written from publicly documented user-visible behaviour of common manga production workflows (print manuscripts, panels, screentones, effect lines, speech balloons), in Efude's own terms. No other application's files, code, or internals were examined, and no third-party material is included. Names used here are Efude's own.

## Phases

| Phase | Scope | Status |
| --- | --- | --- |
| 1 | Page setup and guides, panel borders, screentone layers, focus and speed lines | Implemented |
| 2 | Text (horizontal and vertical), speech balloons with tails, balloon editing | Implemented |
| 3 | Multi-page books, spreads, page numbers, print export with marks, monochrome output | Implemented |

## 1. Page setup

- A manga page is described by its **finished (trim) size** in millimetres, a **bleed** that extends the drawing area beyond the trim, an **inner frame** (the area where panels, dialogue and important drawing are kept) given as margins from the trim edge (top, bottom, binding side, outer side), a resolution in dpi, and the binding side (right-bound for right-to-left reading, left-bound for left-to-right).
- The canvas covers the trim plus bleed on every side. All geometry is converted to canvas pixels at the page resolution.
- Guides are shown over the canvas and never printed or exported: bleed edge, trim edge, inner frame, and crop marks (corner and centre marks) drawn at the trim corners and edge centres.
- Presets cover common cases (a contest manuscript with a B5 finished size at 600 dpi, doujinshi B5 and A5, and a colour B5 page at 350 dpi); every value can be changed.

## 2. Panels

- A **panel** is a convex polygon in canvas pixels with a border width. Each panel owns a folder; layers inside the folder are clipped to the panel shape, so drawing never spills outside the panel. Layers placed above the panel folders (outside them) are not clipped, which is how art breaks out of a panel.
- The border is drawn inside the panel edge (so the clip never cuts it), on a locked layer inside the panel folder, and is regenerated whenever the panel changes.
- **Create panel**: one panel filling the inner frame (or the whole trim, or the current selection's bounding box).
- **Split panel**: drag a line across a panel; the panel is cut along the line and the two parts are pulled apart by the gutter width (horizontal and vertical gutters are set separately and chosen by the direction of the cut). Diagonal cuts are allowed.
- **Grid split**: split a panel into columns × rows with the gutters.
- The panel folder stays editable: splitting keeps the drawing layers of the original panel in the first part and adds an empty drawing layer to the second.
- Border width and the gutters are page settings; changing the border width redraws all borders.

## 3. Screentones

- Any raster layer can be marked as a **tone layer**. Its content is not shown directly: each pixel's darkness × alpha is the tone density, and the layer is displayed as a halftone pattern in the tone colour. The pattern is computed from document coordinates, so dots line up across the whole page.
- Tone parameters (editable at any time, non-destructive): screen frequency in lines per inch, angle, dot shape (round, square, diamond, line, cross, noise), and colour. A density slider sets the density used when filling.
- **Tone fill**: a new tone layer filled at a chosen density inside the selection (or the whole panel/page).
- **Layer to tone**: turns an existing layer into a tone layer so grey drawing becomes dots (gradients become graded dots).
- Because the density comes from ordinary pixels, tones are edited by painting (erase to remove, paint grey to add, gradients for graded tone).
- Export and composite use the same halftone, so what is on screen is what is written.

## 4. Effect lines

- **Focus lines** radiate towards a centre region (an ellipse). Parameters: number of lines, spacing randomness, line length and its randomness, base width, taper toward the centre, and grouping (lines in bundles with gaps between bundles).
- **Speed lines** are parallel lines at an angle across a region. Parameters: number of lines, spacing randomness, length and its randomness, width, taper at both ends.
- Lines are generated on a new layer as tapered wedges (anti-aliased), in the current colour. The region is the selection's bounding box, otherwise the panel under the inner frame, otherwise the canvas.
- A random seed makes a result reproducible; changing it gives a new arrangement.

## 5. Text and balloons

- Text is laid out horizontally (lines centred on each other) or vertically (characters top to bottom, columns right to left), using fonts installed on the system; no fonts are bundled. Size is set in points and converted with the page resolution; line and letter spacing are in ems.
- In vertical text, punctuation, brackets, dashes and ellipses use the Unicode vertical presentation forms when the font has them; otherwise the long vowel mark, dashes and brackets are turned 90°, the comma and full stop move to the upper right of their cell, and small kana are nudged up and right.
- A **balloon** is a shape (ellipse, rounded box, cloud with round bumps, spiky flash, or none for plain text) with a fill, an outline width and colour, and any number of tails. A tail is a tapered wedge from inside the balloon to a tip, optionally bent, or a trail of three shrinking bubbles for thoughts. By default the shape is sized to its text (with room so the text stays inside); dragging its corner fixes the size.
- Balloons are stored as data with the document and drawn on a layer; editing any property redraws the layer as one undoable step. Balloons that share a layer merge: the fill is the union of all shapes, and each shape's outline disappears where it lies inside another shape of the same layer, so tails join their balloon seamlessly and overlapping balloons read as one.
- Tools: the balloon tool drags out a balloon (or clicks for one sized to its text), moves balloons, resizes them by the corner handle, moves tail tips by their handles, and Ctrl-drag pulls a new tail out of a balloon. The text tool places plain text. An editor window edits the text and every property.

## 6. Books

- A **book** is a small JSON file (`.efudebook`) next to its pages: a title, the page setup shared by all pages, the list of page files (`.efude`) in reading order, and page-number settings. Pages are ordinary documents and open in canvas tabs.
- Sides: in a right-bound book the first page is a left-hand page; in a left-bound book it is a right-hand page; pages then alternate. Each new page gets the page setup for its side (the binding margin moves). Spreads pair the pages that face each other when the book is open: the first page stands alone, then (2, 3), (4, 5), …, each placed on its side.
- **Page numbers** (nombre) are drawn at export, centred in the bottom margin between the inner frame and the trim, on the outer side, the centre or the binding side; the start number is adjustable and the first page can be left unnumbered.
- **Export for print** writes one PNG per page (or per spread): the trim only, the canvas with bleed, or the canvas on a larger sheet with crop marks drawn around the trim; in colour, grey, or pure black and white at a threshold. Tones print as their dots.
