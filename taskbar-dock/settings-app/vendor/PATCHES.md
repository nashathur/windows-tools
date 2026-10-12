# Vendored windows-reactor patches

Base: `windows-reactor` 0.100.0, preserved under `vendor/windows-reactor`.

- `src/native/winui/bootstrap.rs`: try the Windows CBS runtime family first and the Store runtime family second, both with the reactor's existing minimum version. Remove the reactor-owned install dialog, retain the resolved family/full package name/package path, and expose that information through `App::bootstrap_runtime`.
- `src/native/winui/mod.rs`: export the package-returning bootstrap entry point while retaining the existing internal bootstrap path for `App::run…`.
- `src/app.rs`: add the public app bootstrap method so the executable can bootstrap before starting WinUI and copy CBS `resources.pri` beside its executable.
- `src/element.rs`: expose the WinUI success, caution, and neutral system brushes for semantic widget status indicators; the original public theme-brush set only included critical status.
- `src/generated.rs`, `src/native/winui/generated.rs`, `src/native/winui/bindings.rs`: a `LostFocus`
  event for TextBox and PasswordBox (`on_lost_focus`), wired like Border's `PointerCaptureLost`
  (event ids `TextBoxLostFocus` / `PasswordBoxLostFocus`, descriptors, visit lists, dispatch, the
  native subscription); `IUIElement` gains a typed `LostFocus` / `RemoveLostFocus` vtable slot and
  wrapper using the existing `RoutedEventHandler`. The app commits text on focus loss.
- `src/element.rs`, `src/native/winui/mod.rs`: `AcceleratorKey::Escape` (VirtualKey 27), so a
  card's Esc restores the saved text.
- `src/native/winui/bindings.rs`, `src/native/winui/generated.rs`: `INumberBox` gains a typed
  `SetSpinButtonPlacementMode` slot; every NumberBox is created with its up/down arrows inline.

Not patched: sliders have no key-up event, so the app writes a keyboard-changed slider after
400 ms without change (pointer release and capture loss write at once).
- `src/native/winui/bootstrap.rs` (part of the runtime patch): `GetPackagePathByFullName` returns a
  Win32 status; any non-zero result is an error, and the path is cut to the buffer.
