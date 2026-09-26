<!-- SPDX-FileCopyrightText: 2026 Hakoniwa -->
<!-- SPDX-License-Identifier: MPL-2.0 -->

# WinTab input backend

The optional Windows WinTab backend loads `wintab32.dll` at runtime. It requests a copy of the driver's default system context, enables system cursor mapping and packet messages, then asks for absolute time, button state, screen coordinates, pressure when exposed by the device, and orientation when exposed. The host window receives WinTab packet notifications and retrieves each packet using its serial number. Coordinates are converted from screen pixels to the application client area before they enter the shared pen queue.

If the driver DLL or required entry points are unavailable, Efude reports the problem and returns to normal window input. Windows Ink remains the default backend. No WinTab runtime or tablet driver is bundled.

Implementation follows the public [Wacom WinTab basics](https://developer-docs.wacom.com/docs/icbt/windows/wintab/wintab-basics/) and [Wacom WinTab reference](https://developer-docs.wacom.com/docs/icbt/windows/wintab/wintab-reference/). The API specifies obtaining a default context with `WTInfo`, opening it with `WTOpen`, handling packet messages with `WTPacket`, and closing the context with `WTClose`. The packet layout is derived from the context's requested data mask.
