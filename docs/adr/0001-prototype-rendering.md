# ADR 0001: First executable prototype

The first vertical slice uses eframe/egui with the WGPU renderer and CPU-side raster storage. This keeps the app runnable early while preserving independent engine crate boundaries. Dedicated tiled storage, GPU dab pipelines, native Windows Ink, and full PSD support are follow-up work.
