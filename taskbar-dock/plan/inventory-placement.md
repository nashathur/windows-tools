## C:\Users\thurs\tools\buswidget\rust\src\placement.rs

Based on the provided `placement.rs` file, here are the user-visible behaviors and widget-specific implementations:

### User-Visible Behaviors
*   **Glides into slot after drop** (`glide_to`, `glide_step`)
*   **Zooms in/out during drag** (`zoom_step`)
*   **Hides when no room next to taskbar buttons** (`apply_fit` -> `FitChange::Hide`)
*   **Shows again when room appears** (`apply_fit` -> `FitChange::Show`)
*   **Compact mode animates width change** (`start_resize`, `resize_step`)
*   **Slides smoothly when moving between dock slots** (`glide_to`, `glide_step`)
*   **Jumps instantly if slide is disabled or not supported** (`glide_to` -> `Glide::Jump`)
*   **Stays put while floating (not attached)** (`drag_ended` -> floating branch)
*   **Repositions automatically when neighbors change** (`on_win_event`, `snap_now`)
*   **Closes popup menu on drag start** (`mouse_drag` -> `DragMove::Started`)
*   **Opens popup menu on click release** (`mouse_up` -> `Release::Click`)
*   **Saves floating position to file** (`drag_ended` -> floating branch)
*   **Centers vertically on taskbar** (`move_bar`)
*   **Respects tray boundary limits during drag** (`limits`, `move_bar`)

### Widget-Specific Behaviors (Not Generic Docking)
*   **Logs attachment status to data directory** (`create_bar` -> logs "strip attached..." or "floating...")
*   **Uses specific class name "BusWidgetBar" and title "Bus widget"** (`create_bar`)
*   **Writes floating position to `pos.txt`** (`drag_ended`)
*   **Handles specific timer IDs for zoom, glide, resize, hover, fade** (Constants `TIMER_ZOOM`, `TIMER_GLIDE`, etc.)
*   **Manages "embedded" vs "floating" state explicitly** (`create_bar`, `drag_ended`)
*   **Calls `dock::mark_member` only if embedded** (`create_bar`)

## C:\Users\thurs\tools\taskbar-media\src\placement.rs

Based on the provided `placement.rs` file, here are the user-visible behaviors and widget-specific implementations:

### User-Visible Behaviors
*   **Glides into slot after drop**: The strip animates smoothly to its new position when dropped (unless a jump is forced).
    *   `glide_to`, `glide_step`
*   **Jumps into place on appearance**: When first appearing or reappearing, it moves instantly without animation.
    *   `jump_to`
*   **Hides when no room**: If the space next to taskbar buttons is insufficient, the strip hides itself.
    *   `check_fit`, `apply_fit` (Hide case)
*   **Shows again when room returns**: When space becomes available, it reappears and fades in.
    *   `apply_fit` (Show case)
*   **Stays put during drag initiation**: While the user is dragging the strip, it slides to follow the mouse cursor rather than snapping to a slot immediately.
    *   `mouse_drag`, `put` (with `raise`)
*   **Raises above neighbors while dragged**: The strip moves to the top of the z-order stack when being dragged.
    *   `put` (with `raise`)
*   **Zooms content while dragging**: The strip's internal content grows slightly when picked up.
    *   `mouse_drag`, `drag_ended`
*   **Returns to normal size after drop**: Upon releasing the mouse, the strip shrinks back to its original width while gliding into place.
    *   `drag_ended`, `zoom_step`
*   **Slides when neighbor moves**: If a neighboring dock member moves, the strip slides to adjust its position (unless pressed).
    *   `reposition_strip`
*   **Stays in current slot if taskbar list is unavailable**: If the system cannot determine the correct dock order, it remains where it currently sits.
    *   `slot`
*   **Does not move while mouse button is held down (pressed)**: Movement logic skips updates if the user is actively pressing the mouse button on the strip.
    *   `check_fit`, `reposition_strip`

### Widget-Specific Behaviors (Not Generic Docking)
*   **Hooks into Windows Explorer events**: It registers specific WinEvent hooks to detect when other taskbar members or the tray change, triggering immediate repositioning.
    *   `on_win_event`, `update_hooks`
*   **Manages floating vs. attached states explicitly**: Logic specifically checks if the strip is "floating" (no dock place) versus "attached" to determine if it can move into a slot or must stay fixed.
    *   `attached`, `mouse_drag`
*   **Closes hover-opened panels on drag start**: When the user starts dragging the strip, any panel that was hovering over it is closed.
    *   `mouse_drag`
*   **Saves new order before ending drag**: It explicitly saves the new docking order to the shared library before finalizing the drop operation.
    *   `drag_ended`
*   **Centers under mouse during zoom**: While dragging, the strip's position calculation attempts to center the content under the cursor within the allowed limits.
    *   `zoom_step`

