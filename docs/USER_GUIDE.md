<!-- SPDX-FileCopyrightText: 2026 Hakoniwa -->
<!-- SPDX-License-Identifier: MPL-2.0 -->

# Efude User Guide

Efude is an early Windows-first painting application. This guide covers the current prototype; some advanced brush and tablet behavior may vary by device.

## Install

Use the Windows MSI to install Efude through the setup wizard. The portable ZIP can be extracted to a folder and launched with `efude.exe`; it does not need a separate installer. Release downloads include license and third-party notice files.

## Start a drawing

1. Launch Efude and use **New** to create a canvas. Set its pixel dimensions and resolution.
   **File → Open** opens Efude documents, PSD files and images alike (it tells them apart by type). A PNG, JPEG, BMP, or GIF opens as a single raster layer and can then be saved as `.efude`.
2. Select a brush from the left panel. Set its size and color in the tool controls, then drag on the canvas to draw.
3. Use the layer panel on the right to add raster layers or folders, change visibility and opacity, reorder or lock layers, and edit masks.
   New layers go just above the selected one. Drag a layer by its row to move it: drop it on the upper or lower edge of another row to put it above or below, or on the middle of a folder row to put it inside that folder (on top of its contents). The picture always stacks in the order the panel shows, top row in front.
   **+ Vector Layer** adds a vector layer: its lines are kept as lines. Draw on it with the brush tools or the rulers (the line takes the colour, size, pressure and opacity of the brush). The eraser cuts away the parts of lines it touches, or with **Vector: erase whole lines it touches** in the tool panel removes each line it touches. Move and Transform move and scale the lines themselves (with a selection, the lines mostly inside it), and Delete or Cut erases the selected parts of the lines. Fill, filters, paste, blur, smudge and mesh transform need pixels: use **Rasterize** in the layer's settings to turn it into a raster layer first.
   Lines on a vector layer are kept as Bézier curves. With the **Control Points** tool (below Move), click a line to select it: its anchors appear as squares (corners as diamonds) with their handles as circles. Drag an anchor to move it, a handle to bend the curve (the opposite handle turns with it unless the anchor is a corner), or the line itself to move all of it. The tool panel changes the selected line's width and color and turns an anchor into a corner or back; Delete removes the selected anchor, or the line. Each change is one Undo step.
4. Choose a selection tool before drawing when edits should affect only part of the image. Selection operations support replace, add, subtract, and intersect modes; feathering, expansion, and shrinking are available in the selection controls.

The in-app Help window describes the current shortcuts. Shortcuts can be changed in the tool panel. Undo and redo include drawing and supported layer or selection edits.

## Brush and canvas controls

- The standard brush set includes pen, pencil, brush, watercolor, airbrush, blur, smudge, and eraser.
- Brush settings include stabilization, pressure curves, size and opacity dynamics, taper, tip and grain controls, scatter, and wet-mixing parameters (bleed style and wet edge). The watercolor and blender presets drag paint into smooth gradients; the wet edge appears when the pen is lifted.
- Use the color controls for the color wheel, sliders, palette, and intermediate color. The eyedropper can sample the selected layer or a composite, according to its settings.
- **Transparent** (the checkerboard swatch under the color wheel) paints with transparency: every brush, ruler and the fill tool then erase, in the brush's own shape, softness and texture. Picking any color, from the wheel, sliders, palette or eyedropper, turns it off.
- Transparent parts of the picture show a checkerboard on screen, so a picture with a transparent background is easy to tell apart from white. Turn it off with **Checkerboard Behind Transparency** in the view settings. Exports are not affected: PNG and PSD keep their transparency.
- The canvas is shown exactly as stored: at 100% one document pixel is one screen pixel (whatever the Windows display scaling), zoomed in the pixels stay sharp squares, and zoomed out the picture is reduced smoothly so thin lines and tones do not break up. The view settings show the zoom in percent with 25–400% and Fit buttons.
- Zoom with the mouse wheel. Move the view with the pan tool, by holding Space, or by dragging with the middle mouse button, anywhere in the canvas area. Canvas rotation, horizontal or vertical mirroring, grid snapping, symmetry, and the navigator are available from the canvas and tool controls.
- Tools are on the left tool bar. The **Tool** panel (below the brushes, beside the color panel) shows only the current tool's settings; choosing a tool brings it to the front, as does the "Tool" entry in the menu bar. View, grid, symmetry and sub view settings are under View → View, Grid and Symmetry….
- The pen, eraser, blur and smudge tools each keep their own brush with its own size and settings: switching tools switches brushes, so changes made to the eraser never change the pen. Clicking a preset selects the tool that draws with it. Selection constrains painting, erasing, and supported filters. Transform operations include scaling, rotation, and mesh warp.
- **Filter** menu: each item opens a window with its settings, a before/after preview of the part of the selected layer in view (at 100%), and **Apply**; the result applies to the selected layer (inside the selection, if any) and one Undo reverts it. While a filter, load or save is working a spinner shows over the canvas. Filters: **Blur** (Gaussian, Lens, Smooth that keeps edges, Motion with a direction; amount in pixels), **Sharpen** (amount and radius), **Reduce Noise**, **Tone Curve** (a smooth curve through five points), **Levels**, **Hue, Saturation, Brightness**, **Auto Levels**, **Color Grading** (15 looks with a strength), **Chromatic Aberration** (horizontal, vertical or diagonal, with an amount), **Mosaic**, **Add Noise** (color or monochrome), and **Line Width** (thicken or thin lines).
- Tapping a tool shortcut key switches tools. Holding it switches only while it is held, and the previous tool comes back on release.
- Right-click a brush preset to duplicate, export, or delete it.
- **Brush Settings…** (in the brush panel, or right-click a preset) opens every setting of the brush in its own window. Its **Stroke feel** switch decides whether a line is reshaped after it is drawn (the tip preview, the end taper and the wet edge at pen-up); turned off, every part of the line is final the moment it appears. Every setting in this window belongs to that one brush. At the top are quick switches: stroke feel, **Wet edge**, **Pressure → size**, **Pressure → density**, and **Anti-aliasing** in four levels (None, Light, Normal, Strong; None gives hard pixel edges). The eraser no longer changes density with pressure by default.
- The brush pressure curve is a graph: drag its two points to shape it. A second curve for the whole application is in File → Preferences…; pen pressure passes through it first, then through the brush's own curve.
- Each brush has its own grain. The grain **Template** list offers four seamless textures (clouds, canvas weave, watercolor paper, chalk). **Grain position** is **Fixed to canvas** (the texture stays put like paper), **Fixed, turned each stroke** (fixed while you draw, but every new stroke turns it by a random angle so layered strokes do not repeat the same pattern), or **Follows the brush** (the texture moves with every dab, like a stamp).
- The selection outline is drawn as moving black-and-white dashes. Moving a selection with the move tool carries only the selected pixels: whatever it passes over or lands on stays where it is (the moved pixels are laid on top).
- Common shortcuts: Ctrl+A select all, Ctrl+D deselect, Ctrl+Shift+I invert selection, Ctrl+C / Ctrl+X / Ctrl+V copy, cut and paste (also images from other apps), Ctrl+Z undo, Ctrl+Y or Ctrl+Shift+Z redo, Ctrl+S save, Ctrl+Shift+S save as, Ctrl+N new, Ctrl+O open, Ctrl+W close tab, Ctrl+Tab next tab, Ctrl+Shift+N new layer, Ctrl+J duplicate layer.
- Brush presets are laid out in columns of ten, as many columns as the panel is wide. Each preset is its own brush: rename it from its right-click menu, the **Name** field of Brush Settings, or **Name** in the Tool panel, and set it up as you like. **Export Brush Set…** (brush panel, Import/Export) saves all presets in one `.efudebrushes` file; placed at `assets/brushes/default.efudebrushes` it becomes the presets of a new installation. **Add Default Brushes** adds one column of default brushes and keeps yours; **Import Brush Set…** and **Restore Default Presets** replace the presets.
- The dotted circle next to the Help menu deletes all layers, leaving one empty layer; Undo brings them back.
- Ctrl and + (on Japanese keyboards also Ctrl and the ";+" key, and the keypad +) zoom the canvas in, Ctrl − zooms out, Ctrl 0 fits it.
- The initial layout has the layer and brush panels as tabs on the right, with the color and tool panels side by side below them.
- Grain images paint where they are black and leave white and transparent parts unpainted. **Invert** swaps the two; **Two levels** paints only on the marked parts (grain amount 1).
- Ctrl + / Ctrl − zoom the canvas and Ctrl 0 fits it; the interface itself keeps its size.
- The **Ink Pen** (ペン画) preset changes only its width with pressure; the line keeps full density. Brush sizes go up to 1000 px for high-resolution pages.
- Each new or opened document gets its own tab above the canvas (an untouched blank canvas is reused). Click a tab to switch, × to close it (you are asked first if it has unsaved changes), + for a new canvas.
- Delete (or Backspace) clears the selected pixels, or the whole layer when nothing is selected; Esc cancels half-placed polygon or curve points. File → New… opens a dialog with size presets.
- Panels (layers, brushes, color, navigator, canvas size) can be dragged by their tabs to stack, split, or float them. The layout is remembered; View → Reset Layout restores the initial one.

## Manga pages

- **Manga → Page Setup…** creates a manga page from a preset (contest B5 at 600 dpi, doujinshi B5/A5, colour B5) or your own trim size, bleed, inner-frame margins, resolution, binding side, and left/right page. The bleed (red), trim (blue), inner frame (cyan), and crop marks are drawn as guides; **Show Guides** hides them.
- **New Panel (Inner Frame)** makes one panel over the inner frame; with a selection, **New Panel (Selection)** uses the selection's bounds. Each panel is a folder whose mask is the panel shape, with a drawing layer and a locked border layer, so anything drawn in it stays inside.
- The **Split Panel** tool (bottom of the tool bar) splits the panel under a line you drag across it, leaving the row or column gap. Nearly level or upright lines snap straight; Shift snaps to 45°. **Grid Split…** splits the selected panel into columns × rows, numbered from the reading start (right to left for right-bound books). **Borders and Gutters…** sets the border width and gaps.
- **Tone Fill…** adds a screentone layer over the selection, else the current panel, else the page, with a density, lines per inch, angle, dot shape (round, square, diamond, line, cross, noise), and colour. A tone layer's pixels are its density: paint or erase on it in grey to change where and how dark the dots are. Its settings can be changed at any time in the layer panel; **Layer to Tone** turns any layer into a tone. Tones are flattened to dots in PSD export.
- **Focus Lines…** and **Speed Lines…** draw effect lines in the current colour on a new layer, fitting the selection, current panel, or inner frame. Adjust count, centre, randomness, width, taper, and bundles, then **Redraw** or **New Arrangement**.
- **Balloons and text**: the Balloon tool (tool bar, bottom) drags out a speech balloon, or a click makes one sized to its text; the Text tool places plain text. The editor window takes the text (vertical or horizontal), font (any installed font), size in points, spacing, colours, the shape (ellipse, rounded box, cloud, spiky, none), outline width, fill, and tails. Drag a balloon to move it, its corner square to resize it, a tail's circle to move the tip; Ctrl-drag from a balloon pulls out a new tail. **Join Overlapping Balloon** puts two balloons on one layer so their outlines merge.
- **Books**: **Manga → Book (Pages)…** creates a folder of page files with the current page setup (sides alternate with the binding), or opens an existing `.efudebook`. It shows the pages as spreads (click one to open it in a tab), lets you add, reorder and remove pages, sets page numbers, and exports every page or spread as PNG: trim only, with bleed, or with crop marks; colour, grey, or black and white. Save open pages first: export reads the saved files.

## Save, export, and backups

- **File → Export…** writes PNG, JPEG or a layered PSD: choose the type in the save dialog (or type the extension). Preferences are at the bottom of the File menu; the interface language (日本語 / English) is in the Help menu.
- Save working documents as **`.efude`**. This format preserves editable layers, masks, and Efude-specific settings.
- Use **PSD** to exchange raster artwork. PSD cannot represent every Efude property; the export reports properties it cannot preserve. Unsupported PSD structures may be imported as the merged image with a warning.
- Export a transparent **PNG** or a white-background **JPEG** for flattened images.
- Efude can periodically back up a saved document. Configure the interval and number of generations in settings. Backups are stored in a `.efude-backups` folder beside the document.
- When working with layers, save to `.efude` regularly. Use PSD as an interchange format, not as the only copy of an editable Efude document.

## Input and troubleshooting

- Windows Ink is the default tablet input. WinTab is an optional runtime backend when a compatible driver is installed. Standard window input remains available as a fallback.
- If pressure or tilt is missing, check the selected input backend and the tablet driver's support. Restart Efude after changing tablet backend settings.
- If GPU compositing is unavailable or fails, Efude falls back to CPU compositing. Some tools and transformed canvas views currently use CPU processing and may be slower on large documents.
- The response-time panel reports input-to-GPU-queue-completion measurements for the current session. It does not include display scan-out time, so it is not a complete measure of perceived pen-to-screen latency.

## License and source code

Efude is free software. The application (`efude-ui`, `efude-app`) is under the Mozilla Public License 2.0 (`LICENSE-MPL`), and the engine crates under MIT OR Apache-2.0 (`LICENSE-MIT`, `LICENSE-APACHE`). The libraries it uses are listed with their licenses in `THIRD_PARTY_NOTICES.txt`.

The source code is at https://github.com/852wa/Efude. Every release is built from a version tag (for example `v0.1.0`), and the source of that exact version can be downloaded from the release page or from `https://github.com/852wa/Efude/archive/refs/tags/<tag>.zip`. `SOURCE.txt`, next to this guide, names the tag and commit this build was made from, with direct links.

## ユーザーガイド

EfudeはWindowsを優先して開発している初期版のペイントソフトです。このガイドは現在の試作版の操作を説明します。高度なブラシやタブレットの挙動は機種によって異なる場合があります。

### インストール

Windows MSIを使うとセットアップウィザードからインストールできます。ポータブルZIPはフォルダーに展開して `efude.exe` を起動します。別途インストーラーは必要ありません。配布物にはライセンスとサードパーティ告知のファイルが含まれます。

### 描画を始める

1. Efudeを起動し、「新規」からキャンバスを作成します。ピクセル寸法と解像度を指定します。
   「ファイル → 開く」でEfude形式・PSD・画像のどれでも開けます（種類はファイルから自動で判断します）。PNG、JPEG、BMP、GIFは1枚のラスターレイヤーとして開き、`.efude`形式で保存できます。
2. 左パネルでブラシを選び、ツール設定でサイズと色を決めて、キャンバス上をドラッグして描きます。
3. 右のレイヤーパネルからラスターレイヤーやフォルダーを追加し、表示、不透明度、順序、ロック、マスクを操作します。
   新しいレイヤーは選択中のレイヤーのすぐ上に入ります。レイヤーの行をつかんでドラッグすると移動できます。ほかの行の上端・下端に落とすとその上・下へ、フォルダーの行の中ほどに落とすとそのフォルダーの中（中身のいちばん上）に入ります。キャンバスの重なり順は常にパネルの並びと同じで、上の行ほど手前です。
   「＋ベクター」でベクターレイヤーを追加します。描いた線を線のまま持つレイヤーで、ブラシツールや定規で描けます（線にはブラシの色・サイズ・筆圧・不透明度が使われます）。消しゴムは触れた部分だけ線を消し、ツールパネルの「ベクター: 触れた線をまるごと消す」をオンにすると触れた線を1本まるごと消します。移動・変形は線そのものを動かし、拡大縮小します（選択範囲があれば、大半がその中にある線だけ）。Deleteや切り取りは選択範囲内の線の部分を消します。塗りつぶし、フィルター、貼り付け、ぼかし・指先、メッシュ変形は画素が必要なため、レイヤー設定の「ラスタライズ」でラスターレイヤーにしてから使ってください。
   ベクターレイヤーの線はベジェ曲線として保持されます。「制御点」ツール（移動の下）で線をクリックすると選択され、アンカーが□（角は◇）、ハンドルが○で表示されます。アンカーをドラッグすると点が動き、ハンドルをドラッグすると曲がり方が変わります（角でないアンカーは反対側のハンドルも一緒に回ります）。線そのものをドラッグすると線全体が動きます。ツールパネルで選んだ線の太さ・色の変更、アンカーの角／なめらかの切り替えができ、Deleteで選んだアンカー（選んでいなければ線）を消します。どの操作も1回で取り消せます。
4. 一部分だけ編集するときは、描画前に選択ツールで範囲を作ります。選択範囲には置換、追加、減算、交差のモードがあり、境界のぼかし、拡張、縮小もできます。

現在のショートカットはアプリ内のヘルプで確認できます。ツールパネルから変更できます。取り消し／やり直しは描画と対応するレイヤー・選択範囲の編集に適用されます。

### ブラシとキャンバス

- 標準ブラシはペン、鉛筆、筆、水彩、エアブラシ、ぼかし、指先、消しゴム、色混ぜです。
- 水彩と色混ぜは、塗った色から別の色へ引きずるとなめらかなグラデーションになります。筆圧が軽いほど少しずつ混ざります。「にじみ方」（ふんわり／ふつう／しっかり）で透明部分への薄まり方が、「にじみ縁」でペンを離したときに縁にたまる色の濃さが変わります。以前の設定を使っている場合は「このブラシを初期設定に戻す」で新しい水彩に戻せます。
- ブラシ設定には手ブレ補正、筆圧カーブ、サイズ・不透明度の入力反応、入り抜き、先端とグレイン、散布、ウェット混色があります。
- カラー操作ではカラーホイール、スライダー、パレット、中間色を使えます。スポイトは設定に応じて選択レイヤーまたは合成画像から色を拾います。
- カラーホイール下の市松模様の「透明色」をオンにすると透明色で描けます。どのブラシ・定規・塗りつぶしも、そのブラシの形・やわらかさ・質感のまま消す動作になります。ホイール・スライダー・パレット・スポイトで色を選ぶとオフに戻ります。
- 絵の透明な部分は画面上で市松模様（チェッカー）で表示されるので、背景が透明な絵と白い背景を見分けられます。表示設定の「透明部分を市松模様で表示」でオフにできます。書き出しには影響せず、PNGやPSDは透明のまま保存されます。
- キャンバスは保存されている画素のまま表示されます。100%では1ピクセルが画面の1ピクセルになり（Windowsの表示スケールに関係なく）、拡大するとピクセルはくっきりした四角のまま、縮小すると細い線やトーンが途切れないようになめらかに縮小されます。表示設定で倍率を％で確認でき、25〜400%と「全体」のボタンがあります。
- マウスホイールで拡大縮小します。表示位置は、手のひらツール、スペースキーを押しながらのドラッグ、中ボタンのドラッグで移動できます。キャンバスの外側からでも動かせます。キャンバスの回転・反転、グリッド吸着、対称定規、ナビゲーターも利用できます。
- ツールは左のツールバーにあります。「ツール」パネル（ブラシの下、カラーの隣）には今のツールの設定だけが表示され、ツールを選ぶと前面に出ます（メニュー行の「ツール」からも出せます）。表示・グリッド・対称定規・サブビューの設定は「表示 → 表示・グリッド・対称定規…」にあります。
- ペン・消しゴム・ぼかし・指先の各ツールは、それぞれ自分のブラシ（サイズと設定を含む）を覚えています。ツールを切り替えるとブラシも切り替わるので、消しゴムの調整がペンに影響することはありません。プリセットをクリックすると、そのブラシで描くツールに切り替わります。選択範囲は描画、消去、対応フィルターの適用範囲を制限します。変形には拡大縮小、回転、メッシュ変形があります。
- 「フィルター」メニューの項目を選ぶと、その設定ウィンドウが開きます。表示中の場所を等倍で「適用前／適用後」でプレビューし、「適用」で選んでいるレイヤー（選択範囲があればその中）に適用します。元に戻す1回で戻せます。フィルターや読み込み・保存の処理中は、キャンバスの上に「処理中…」の表示が出ます。フィルター: ぼかし（ガウス・レンズ・輪郭を残すスムーズ・方向を指定する移動、量はpx）、シャープ（量と半径）、ノイズ除去、トーンカーブ（5つの点を通るなめらかな曲線）、レベル補正、色相・彩度・明度、自動レベル補正、カラーグレーディング（15種類と強さ）、色収差（横・縦・斜めと量）、モザイク、ノイズ（カラー・モノクロ）、線の太さ（太らせる・細らせる）。
- ツールのショートカットキーは、一度押すとそのツールに切り替わります。押し続けている間だけ切り替え、離すと元のツールに戻ることもできます。
- ブラシプリセットを右クリックすると、複製・書き出し・削除ができます。
- ブラシパネルの「ブラシの詳細設定…」（またはプリセットの右クリック）で、そのブラシの全設定を別ウィンドウで調整できます。「書き味」の「描いたあとで線を整える」をオフにすると、線は描いたその場で確定し、先端の仮表示・終わりの入り抜き・ペンを離したときのにじみ縁によって、あとから線が変わることがなくなります。このウィンドウの設定はすべてそのブラシだけのものです。上部に「にじみ縁」「筆圧で太さ」「筆圧で濃さ」のオンオフと、アンチエイリアス4段階（なし／弱／中／強。なしはピクセルのくっきりした縁）があります。消しゴムは初期状態で筆圧による濃さの変化がありません。
- ブラシの筆圧カーブはグラフの2つの点をドラッグして調整します。アプリ全体の筆圧カーブは「ファイル → 環境設定…」にあり、ペンの筆圧はまずこちらを通ってから各ブラシのカーブに渡ります。
- グレインはブラシごとの設定です。「テンプレート」から継ぎ目のない4種類の模様（雲・布目・水彩紙・チョーク）を選べます。「グレインの位置」は「キャンバスに固定」（紙の目のように模様が動かない）、「固定・描くたびに回転」（描いている間は固定し、線を引くたびに模様の向きがランダムに変わるので重ね塗りで同じ模様が目立たない）、「ブラシに追従」（一打ごとに模様がブラシと動くスタンプ風）から選びます。
- 選択範囲の境界は白黒の動く破線で表示されます。移動ツールで選択範囲を動かすと、選んだ部分だけが移動し、通り道や移動先にある色は引きずられず、そのまま残ります（移動した部分がその上に重なります）。
- 一般的なショートカット: Ctrl+A すべてを選択、Ctrl+D 選択解除、Ctrl+Shift+I 選択範囲を反転、Ctrl+C／Ctrl+X／Ctrl+V コピー・切り取り・貼り付け（他のアプリの画像も貼り付け可）、Ctrl+Z 取り消し、Ctrl+Y または Ctrl+Shift+Z やり直し、Ctrl+S 保存、Ctrl+Shift+S 別名で保存、Ctrl+N 新規、Ctrl+O 開く、Ctrl+W タブを閉じる、Ctrl+Tab 次のタブ、Ctrl+Shift+N 新規レイヤー、Ctrl+J レイヤーを複製。
- ブラシプリセットは10個ずつの列で並び、パネルの幅に入るだけ列が増えます。プリセットはそれぞれ独立したブラシです。右クリックメニューの「名前を変更…」、ブラシの詳細設定の「名前」、ツールパネルの「名前」で名前を付け、自由に設定できます。ブラシパネルの「読み書き」にある「ブラシセットを書き出し…」でプリセット一式を `.efudebrushes` ファイルに保存でき、これを `assets/brushes/default.efudebrushes` に置くと新しくインストールしたときの初期プリセットになります。「初期ブラシを追加」は今のブラシを残したまま初期ブラシを1列追加します。「ブラシセットを読み込み…」「初期プリセットに戻す」はプリセットを置き換えます。
- ヘルプの横の点線の丸は「レイヤー全削除」です。空のレイヤーを1枚残してすべて削除します（元に戻すで戻せます）。
- Ctrl＋＋（日本語キーボードでは Ctrl＋「;+」キー、テンキーの＋も可）で拡大、Ctrl＋−で縮小、Ctrl＋0で全体表示します。
- 初期配置では、右側にレイヤーとブラシのタブ、その下にカラーとツールのパネルが横に並びます。
- グレイン画像は、黒い所が塗られ、白と透明の所は塗られません。「反転」で入れ替え、「二値化」で判定のある所だけを塗ります（グレイン量は1になります）。
- Ctrl＋＋／Ctrl＋−でキャンバスを拡大縮小、Ctrl＋0で全体表示します（画面全体の文字などの大きさは変わりません）。
- 「ペン画」プリセットは筆圧で太さだけが変わり、線の濃さはいつも一定です。ブラシサイズは高解像度の原稿向けに1000pxまで大きくできます。
- 新しいキャンバスや開いたファイルは、キャンバス上のタブに1つずつ開きます（何も描いていない新規キャンバスはそのまま使われます）。タブをクリックで切り替え、×で閉じます（保存していない変更があれば確認します）。＋で新規キャンバスを作ります。
- Delete（またはBackspace）キーで選択範囲の中を消去します。選択していないときはレイヤー全体を消去します。Escキーで置きかけの多角形や曲線の点を取り消します。「ファイル → 新規…」では用紙サイズを選んで新しいキャンバスを作れます。
- レイヤー、ブラシ、カラー、ナビゲーター、用紙の各パネルは、タブをドラッグして重ねる・分割する・別ウィンドウにすることができます。配置は保存され、「表示 → レイアウトを初期状態に戻す」で最初の配置に戻せます。

### 漫画原稿

- 「漫画 → 原稿の設定…」で、プリセット（投稿用B5 600dpi、同人誌B5／A5、カラーB5）または仕上がりサイズ・裁ち落とし・基本枠の余白・解像度・綴じ方向・左右ページを指定して漫画原稿を作ります。裁ち落とし（赤）、仕上がり（青）、基本枠（水色）、トンボがガイドとして表示されます。「ガイドを表示」で隠せます。
- 「コマ枠を作成（基本枠）」で基本枠いっぱいのコマを作ります。選択範囲があれば「コマ枠を作成（選択範囲）」でその範囲に作れます。コマはマスク付きのフォルダーで、中に作画レイヤーとロックされた枠線レイヤーが入ります。コマの中で描いたものはコマからはみ出しません。
- ツールバー下の「コマ分割」ツールでコマを横切るようにドラッグすると、その線でコマを分割し、上下・左右の間隔を空けます。ほぼ水平・垂直の線はまっすぐに、Shift を押すと45°刻みになります。「コマを等分割…」は選んでいるコマを横×縦に分け、読み始め側（右綴じなら右）から番号を付けます。「枠線と間隔…」で枠線の太さと間隔を変えられます。
- 「トーンを貼る…」で、選択範囲（なければ選んでいるコマ、なければページ全体）にトーンレイヤーを作ります。濃度、線数、角度、網点の形（円・四角・ひし形・線・十字・砂目）、色を選べます。トーンレイヤーの画素は濃度を表すので、グレーで描いたり消したりすると網点の範囲や濃さが変わります。設定はレイヤーパネルでいつでも変えられ、「レイヤーをトーンにする」で普通のレイヤーもトーンにできます。PSD書き出しでは網点として統合されます。
- 「集中線…」「流線…」は、選択範囲・選んでいるコマ・基本枠に合わせて、描画色で新しいレイヤーに効果線を描きます。本数、中心、乱れ、太さ、入り抜き、まとまりを調整して「描き直す」「配置を変える」を押します。
- 「フキダシ」ツール（ツールバー下）でドラッグするとフキダシを、クリックすると文字に合わせた大きさのフキダシを作ります。「テキスト」ツールは文字だけを置きます。編集ウィンドウでセリフ（縦書き・横書き）、フォント（パソコンに入っているフォント）、文字サイズ（pt）、行間・字間、色、形（楕円・角丸・雲・トゲ・なし）、線の太さ、塗り、しっぽを変えられます。フキダシをドラッグで移動、右下の□で大きさ、しっぽ先の○で向きを変えます。Ctrl+ドラッグでしっぽを追加します。「重なるフキダシとつなげる」で2つのフキダシを同じレイヤーにすると、線がつながります。
- 「漫画 → 作品（複数ページ）…」で、今の原稿の設定を使ってページのファイルをフォルダーにまとめて作ります（綴じ方向に合わせて左右ページが交互になります）。既存の `.efudebook` も開けます。見開きでページを一覧し（クリックでタブに開く）、ページの追加・並べ替え・外す、ノンブル（ページ番号）の設定、全ページまたは見開きのPNG書き出し（仕上がり・裁ち落としまで・トンボ付き、カラー・グレー・モノクロ2階調）ができます。書き出しは保存済みのファイルを使うので、開いているページは先に保存してください。

### 保存、書き出し、自動バックアップ

- 「ファイル → 書き出し…」でPNG・JPEG・レイヤー付きPSDに書き出します。保存ダイアログでファイルの種類を選びます（拡張子を入力しても選べます）。環境設定はファイルメニューのいちばん下、表示言語（日本語／English）はヘルプメニューにあります。
- 編集データは **`.efude`** 形式で保存します。レイヤー、マスク、Efude固有の設定を保持します。
- ラスター画像の受け渡しには **PSD** を使えます。PSDがEfudeの全設定を保持できるわけではありません。書き出し時に保持できない情報が警告されます。未対応構造を含むPSDは、警告を出して統合画像として読み込む場合があります。
- 透明部分を保つ **PNG**、または白背景の **JPEG** に書き出せます。
- 保存済みドキュメントは一定間隔で自動バックアップできます。設定で間隔と世代数を変更できます。バックアップはドキュメントと同じ場所の `.efude-backups` フォルダーに作成されます。
- レイヤーを含む作品は、`.efude` 形式で定期的に保存してください。編集データの唯一のコピーにPSDだけを使わないでください。

### 入力とトラブルシューティング

- タブレット入力はWindows Inkが標準です。対応ドライバーがある場合はWinTabも選択できます。通常のウィンドウ入力も代替として利用できます。
- 筆圧や傾きが反映されない場合は、入力方式とタブレットドライバーの対応状況を確認してください。入力方式を変更した後はEfudeを再起動してください。
- GPU合成が利用できない場合や失敗した場合、CPU合成に切り替わります。一部のツールや変形表示中のキャンバスはCPUで処理され、大きな作品では遅くなることがあります。
- 応答速度パネルは、現在のセッションにおける入力からGPUキュー完了までを計測します。画面走査表示時間は含まないため、体感上のペンから画面までの遅延すべてを表す値ではありません。

### ライセンスとソースコード

Efudeは自由ソフトウェアです。アプリ部分（`efude-ui`・`efude-app`）はMozilla Public License 2.0（`LICENSE-MPL`）、エンジン部分はMIT OR Apache-2.0（`LICENSE-MIT`・`LICENSE-APACHE`）です。使用しているライブラリとそのライセンスは `THIRD_PARTY_NOTICES.txt` にあります。

ソースコードは https://github.com/852wa/Efude で公開しています。各リリースはバージョンのタグ（例: `v0.1.0`）から作られており、そのバージョンのソースはリリースページ、または `https://github.com/852wa/Efude/archive/refs/tags/<タグ>.zip` から入手できます。このガイドと同じ場所にある `SOURCE.txt` に、この配布物を作ったタグとコミット、直接のリンクが書かれています。
