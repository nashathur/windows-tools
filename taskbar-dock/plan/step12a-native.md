WinUI provides the real Settings-style controls. Comctl32 provides real classic Windows controls, but visual styles and dark-mode tricks do not turn them into WinUI.

Evidence checked against Microsoft documentation and source repositories. Unverified: actual rendering on 24H2+, Rust builds, runtime installation, and performance; no application or Python was run. “Cost” below means integration and deployment effort.

| Option | Unpackaged Rust exe? | Windows 11 / Fluent look? | Cost |
|---|---|---|---|
| 1. Comctl32 v6 | Yes, directly | Classic OS-themed controls; not Settings-style WinUI | Low for light mode; appreciable custom work for dark |
| 2. System XAML Islands | Yes, with manifest and interop work | Win10-era system XAML; WinUI 2.6+ adds Win11 styles | Medium–high; Rust projection and hosting work |
| 3. WinUI 3 / Windows App SDK | Yes, with runtime initialization/deployment | Yes: real Fluent controls, rounded styles, `ToggleSwitch`, navigation | Highest integration/deployment burden |
| 4. GDI borrowings | Yes | Authentic fonts, icons, backdrop and colors; controls remain yours | Low, except transparent backdrop rendering |

1. Comctl32 v6 and dark mode

Enable the `Microsoft.Windows.Common-Controls` 6.0.0.0 manifest dependency and initialize the needed classes with `InitCommonControlsEx`. This requires no MSIX or extra control runtime. [Microsoft visual-styles guidance](https://learn.microsoft.com/en-us/windows/win32/controls/cookbook-overview).

| Control | Appearance and dark-mode verdict |
|---|---|
| `msctls_trackbar32` | Legacy slider geometry, not the WinUI circular-thumb slider. No complete automatic dark treatment demonstrated; Microsoft’s ZoomIt subclasses and paints it. |
| `BUTTON` push button | OS-themed classic button. `DarkMode_Explorer` can supply dark styling; it still is not a WinUI button. |
| Checkbox/radio/group box | Classic controls. Dark text/background/glyph handling can require custom painting; ZoomIt subclasses these. |
| `EDIT` | Classic textbox, not WinUI’s rounded textbox/underline treatment. Text/background can be colored through `WM_CTLCOLOREDIT`; border treatment may require subclassing. |
| `COMBOBOX` | Classic combo. `DarkMode_CFD` is used by Microsoft, but editable child, dropdown list and scrollbar behavior require checking. |
| Toggle switch | No native comctl32 `ToggleSwitch`. A checkbox with `BS_PUSHLIKE` is a checkable push button. |

The dark-mode implementation evidence is [PowerToys `Utility.cpp`, lines 281–325](https://github.com/microsoft/PowerToys/blob/main/src/modules/ZoomIt/ZoomIt/Utility.cpp#L281), and the toggle distinction follows the [documented button styles](https://learn.microsoft.com/en-us/windows/win32/controls/button-styles). Exact light-mode shapes on 24H2 are visually unverified; none of these routes promises Settings’ WinUI templates.

`SetWindowTheme` itself is documented, but `"DarkMode_Explorer"`/`"DarkMode_CFD"` are undocumented theme conventions. Likewise, uxtheme 133 = `AllowDarkModeForWindow`, 135 = `SetPreferredAppMode` on Windows 10 1903+, and 136 = `FlushMenuThemes` are undocumented. Microsoft uses them, but that does not establish a compatibility guarantee. Ordinal 135 previously had a different signature. [PowerToys lines 33–75](https://github.com/microsoft/PowerToys/blob/main/src/modules/ZoomIt/ZoomIt/Utility.cpp#L33), [historical ordinal definitions](https://github.com/ysc3839/win32-darkmode/blob/master/win32-darkmode/DarkMode.h#L57).

2. System XAML Islands: workable, but not a free Win11 upgrade

`Windows.UI.Xaml.Hosting.DesktopWindowXamlSource` hosts real system XAML controls, including `Slider` and `ToggleSwitch`. The engine ships with Windows. An unpackaged application needs an application manifest: Microsoft’s example includes `<maxversiontested Id="10.0.18362.0"/>` or later and the Windows 10/11 `supportedOS` GUID. This is an ordinary exe manifest, not MSIX. [Manifest/setup requirements](https://learn.microsoft.com/en-us/windows/apps/desktop/modernize/xaml-islands/host-standard-control-with-xaml-islands-cpp).

Rust caveat: `windows` 0.58 does not contain `Windows.UI.Xaml`. Its [`Windows/UI/mod.rs`, lines 1–28](https://github.com/microsoft/windows-rs/blob/0.58.0/crates/libs/windows/src/Windows/UI/mod.rs#L1) has no XAML module. You need custom generated bindings or a C++ interop component; enabling a Cargo feature alone cannot provide this.

Expect Win10-era system templates without WinUI 2. Microsoft places the Win11 style update in WinUI 2.6, including updated styles for existing controls. Exact stock 24H2 rendering is unverified. [WinUI 2.6 release notes](https://learn.microsoft.com/en-us/windows/uwp/get-started/winui2/release-notes/winui-2.6).

WinUI 2 can be hosted unpackaged, conditionally: Microsoft documents a prerelease package route or Dynamic Dependencies for the release package. This adds library deployment, activation and resource setup. System Islands also do not automatically follow live light/dark changes: detect changes and explicitly update the root `FrameworkElement.RequestedTheme`; that property is mutable at runtime. [Island limitations/deployment](https://learn.microsoft.com/en-us/windows/uwp/xaml-islands/xaml-islands#limitations-and-workarounds), [RequestedTheme](https://learn.microsoft.com/en-us/uwp/api/windows.ui.xaml.frameworkelement.requestedtheme).

3. WinUI 3: best match, substantial integration

WinUI 3 supplies Microsoft’s Fluent controls. For incremental migration, use `Microsoft.UI.Xaml.Hosting.DesktopWindowXamlSource`—a different API/runtime from system Islands. Unpackaged hosting is supported; the host owns dispatcher initialization, native bounds, focus, input integration and teardown. [Microsoft’s current Islands guidance](https://learn.microsoft.com/en-us/windows/apps/desktop/modernize/host-controls-existing-desktop-apps).

For framework-dependent deployment, install the matching Windows App SDK runtime and initialize it before WinUI, typically through `Microsoft.WindowsAppRuntime.Bootstrap.dll` → `MddBootstrapInitialize2`, then shut down appropriately. Self-contained deployment bundles the runtime instead and increases the distribution size. Neither runtime availability nor Rust auto-initialization follows merely from running Windows 11. [Deployment guide](https://learn.microsoft.com/en-us/windows/apps/windows-app-sdk/deploy-unpackaged-apps), [bootstrap API](https://learn.microsoft.com/en-us/windows/windows-app-sdk/api/win32/mddbootstrap/nf-mddbootstrap-mddbootstrapinitialize2), [self-contained deployment](https://learn.microsoft.com/en-us/windows/apps/package-and-deploy/unpackage-winui-app).

Rust implementations with source/examples exist:

- Microsoft `windows-reactor`, currently preview, wraps real WinUI 3 controls and documents framework-dependent/self-contained execution. [Guide and samples](https://github.com/microsoft/windows-rs/blob/master/docs/crates/windows-reactor.md).
- `compio-rs/winio` has a WinUI 3 backend and examples. [Project README](https://github.com/compio-rs/winio/blob/master/README.md).
- `Alovchin91/rust-winui` is an older proof of concept with documented binding/composition difficulties. [Author’s implementation report](https://github.com/microsoft/windows-rs/issues/3404).

These are source-backed feasibility examples, not builds I verified, and should not be assumed compatible with your existing `windows` 0.58 types.

My recommendation for this app, in five lines:

1. Choose WinUI 3 if Settings-like controls are a firm requirement; its controls match that design language.
2. Prototype one WinUI 3 Island containing a Slider, ToggleSwitch and ComboBox before committing to migration.
3. Compare Microsoft’s Rust Reactor preview with a C++/WinRT Island component exposing a small C ABI; the latter follows Microsoft’s documented hosting path.
4. If extra runtime deployment is unacceptable, choose comctl32 v6 plus the borrowings below, accepting the classic control appearance.
5. Avoid investing in system Islands plus WinUI 2 solely for styling: it adds deployment/binding work while retaining documented live-theme limitations.

The smaller borrowings available to the GDI app are:

| Borrowing | Exact names / APIs |
|---|---|
| Icons | Font `Segoe Fluent Icons`, shipped with Windows 11. Draw separate glyphs using `CreateFontW`/`CreateFontIndirectW`, `SelectObject`, `DrawTextW` or `TextOutW`. |
| Navigation glyphs | `GlobalNavButton` U+E700; `Home` U+E80F; `Settings` U+E713; `System` U+E770; `Devices` U+E772; `Wifi` U+E701; `Bluetooth` U+E702; `Personalize` U+E771. |
| Arrows | `Back` U+E72B; `ChromeBack` U+E830; `ChevronRight` U+E76C; `ChevronLeft` U+E76B; `ChevronDown` U+E70D; `ChevronUp` U+E70E. |
| Typography | Windows 11 ships `Segoe UI Variable`, with named Text, Small, and Display instances. For GDI, enumerate/select the appropriate installed face and set weights through `LOGFONTW`. Exact GDI face selection is unverified here. |

Glyph names/codepoints are from [Microsoft’s icon table](https://learn.microsoft.com/en-us/windows/apps/design/iconography/segoe-fluent-icons-font); font instances are in the [Windows 11 font list](https://learn.microsoft.com/en-us/typography/fonts/windows_11_font_list).

- Mica: `DwmSetWindowAttribute(hwnd, DWMWA_SYSTEMBACKDROP_TYPE /*38*/, …)` with `DWMSBT_MAINWINDOW = 2`. `DWMSBT_TABBEDWINDOW = 4` selects Mica Alt; `DWMSBT_TRANSIENTWINDOW = 3` selects Desktop Acrylic. Supported since build 22621. Also set `DWMWA_USE_IMMERSIVE_DARK_MODE = 20` for the frame’s dark preference. Opaque client painting can hide the backdrop; exposing it requires suitable frame/alpha handling, potentially `DwmExtendFrameIntoClientArea`. The app’s GDI composition behavior remains unverified. [Backdrop enum](https://learn.microsoft.com/en-us/windows/win32/api/dwmapi/ne-dwmapi-dwm_systembackdrop_type), [DWM attributes](https://learn.microsoft.com/en-us/windows/win32/api/dwmapi/ne-dwmapi-dwmwindowattribute), [frame/alpha requirements](https://learn.microsoft.com/en-us/windows/win32/dwm/customframe).

- Accent palette: `Windows.UI.ViewManagement.UISettings.GetColorValue(UIColorType::{Accent, AccentLight1, AccentLight2, AccentLight3, AccentDark1, AccentDark2, AccentDark3})`. Subscribe to `ColorValuesChanged`, marshal repaint/update work onto the UI thread, and honor high contrast separately. These bindings are present in `windows` 0.58. [Palette enum](https://learn.microsoft.com/en-us/uwp/api/windows.ui.viewmanagement.uicolortype), [0.58 methods, lines 1978–1997](https://github.com/microsoft/windows-rs/blob/0.58.0/crates/libs/windows/src/Windows/UI/ViewManagement/mod.rs#L1978).

- Theme-painted pieces: `OpenThemeData`/`OpenThemeDataForDpi` → `DrawThemeBackground` → `CloseThemeData`. Classes/parts include `BUTTON` / `BP_PUSHBUTTON`, `BP_CHECKBOX`, `BP_RADIOBUTTON`; `TRACKBAR` / `TKP_TRACK`, `TKP_THUMB`; `EDIT` / `EP_EDITTEXT`; `COMBOBOX` / `CP_DROPDOWNBUTTON`. These paint classic theme assets; they do not supply WinUI templates or control behavior. Reopen handles after `WM_THEMECHANGED`. [Parts/states](https://learn.microsoft.com/en-us/windows/win32/controls/parts-and-states), [DPI and handle lifetime](https://learn.microsoft.com/en-us/windows/win32/api/uxtheme/nf-uxtheme-openthemedatafordpi).

- High contrast: `SystemParametersInfoW(SPI_GETHIGHCONTRAST, … HIGHCONTRASTW …)` and `HCF_HIGHCONTRASTON`. Use `GetSysColor`/`GetSysColorBrush` with paired `COLOR_WINDOW`/`COLOR_WINDOWTEXT`, `COLOR_BTNFACE`/`COLOR_BTNTEXT`, `COLOR_HIGHLIGHT`/`COLOR_HIGHLIGHTTEXT`, plus `COLOR_GRAYTEXT`. Refresh cached colors on system/theme changes. [Microsoft high-contrast guidance](https://learn.microsoft.com/en-us/windows/win32/winauto/high-contrast-parameter).

## Suggestions

- Define whether “Windows-provided” permits a Microsoft runtime dependency. WinUI is Microsoft-native but separately deployed; this determines whether option 3 meets the owner’s constraint.
- Gate migration on keyboard/focus, mixed-monitor DPI, live-theme and clean-machine deployment checks. Microsoft explicitly identifies these hosting responsibilities; appearance alone does not validate an Island. [Hosting guidance](https://learn.microsoft.com/en-us/windows/apps/desktop/modernize/host-controls-existing-desktop-apps).
- Keep undocumented dark APIs behind a replaceable adapter. Ordinal signatures have changed historically, so isolating them limits future repair costs. No suggestions were implemented.