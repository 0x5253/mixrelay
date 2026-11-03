"""Exercise the installed WebKit UI through its native accessibility tree."""
from pathlib import Path
from functools import lru_cache
import os
import sys
import time
import traceback

import pyatspi


def children(node, report=None):
    if node is None:
        return
    try:
        count = node.childCount
    except Exception as error:
        if report is not None:
            report(f"[AT-SPI childCount failed: {error}]")
        return
    if report is not None:
        report(f"[AT-SPI childCount={count}]")
    for index in range(count):
        try:
            child = node[index]
        except Exception as error:
            if report is not None:
                report(f"[AT-SPI child {index} failed: {error}]")
            continue
        if child is not None:
            yield child
        elif report is not None:
            report(f"[AT-SPI child {index} returned null]")


def describe(node, report=None):
    try:
        return node.getRoleName(), node.name or "", node.getState().contains(pyatspi.STATE_SHOWING)
    except Exception as error:
        # AT-SPI proxies can disappear between enumeration and inspection.
        if report is not None:
            report(f"[AT-SPI description failed: {error}]")
        return None


def tree_text(root, diagnostic=False):
    lines = []
    report = lines.append if diagnostic else None
    for node, depth in descendants(root, report):
        description = describe(node, report)
        if description is not None:
            role, name, showing = description
            state = f" [showing={showing}]" if diagnostic else ""
            lines.append(f"{' ' * depth}{role} {name}{state}")
    return "\n".join(lines)


def descendants(root, report=None):
    queue = [(root, 0)]
    count = 0
    while queue:
        node, depth = queue.pop()
        if node is None:
            continue
        count += 1
        if count > 20000:
            raise RuntimeError("Installed accessibility tree exceeded its bound")
        yield node, depth
        if depth < 60:
            queue.extend((child, depth + 1) for child in reversed(list(children(node, report))))


@lru_cache(maxsize=1)
def application():
    # Keep the selected editor's identity throughout this one-process UI run.
    for app in children(pyatspi.Registry.getDesktop(0)):
        description = describe(app)
        if description is None or "mixrelay" not in description[1].lower():
            continue
        # The editor and headless runtime have the same AT-SPI application name.
        # Registration order depends on startup timing; select the visible editor.
        for window in children(app):
            description = describe(window)
            if description is None:
                continue
            role, name, showing = description
            if (role == "frame" and name == "MixRelay"
                    and showing):
                return app
    raise LookupError("Installed MixRelay has no visible accessibility editor")


def observation_timeout():
    return 120 if os.environ.get("MIXRELAY_E2E_VM_ACCELERATOR") == "tcg" else 30


def find(label, contains=False, interactive=True, deadline=None, roles=None, text=False, enabled=False):
    if deadline is None:
        deadline = time.monotonic() + observation_timeout()
    while time.monotonic() < deadline:
        try:
            for node, _ in descendants(application()):
                try:
                    name = node.name or ""
                    if text and node.getRoleName() == "paragraph":
                        name = node.queryText().getText(0, -1)
                    matched = label in name if contains else name == label
                    if not matched:
                        continue
                    if roles is not None and node.getRoleName() not in roles:
                        continue
                    if roles is None and interactive and node.getRoleName() not in ("button", "push button", "menu item", "combo box", "list item"):
                        continue
                    state = node.getState()
                    if state.contains(pyatspi.STATE_SHOWING) and (not enabled or state.contains(pyatspi.STATE_ENABLED)):
                        return node
                except Exception:
                    continue
        except (LookupError, RuntimeError):
            pass
        time.sleep(0.1)
    raise AssertionError(f"Missing installed UI control: {label}")


def locate(label, contains=False, interactive=True):
    deadline = time.monotonic() + observation_timeout()
    last = None
    while time.monotonic() < deadline:
        node = find(label, contains=contains, interactive=interactive, deadline=deadline)
        try:
            bounds = node.queryComponent().getExtents(pyatspi.DESKTOP_COORDS)
            if bounds.width > 0 and bounds.height > 0:
                return node, bounds
        except Exception as error:
            last = error
        time.sleep(0.1)
    raise AssertionError(f"Installed control bounds unavailable: {label}: {last}")


def click(label, contains=False, right=False):
    started = time.monotonic()
    print(f"Finding installed UI control: {label}", flush=True)
    _, bounds = locate(label, contains=contains, interactive=not right)
    x, y = bounds.x + bounds.width // 2, bounds.y + bounds.height // 2
    pyatspi.Registry.generateMouseEvent(x, y, "abs")
    pyatspi.Registry.generateMouseEvent(x, y, "b3c" if right else "b1c")
    print(f"Activated installed UI control: {label} ({time.monotonic() - started:.2f}s)", flush=True)


def focus(node, label, deadline):
    try:
        node.queryComponent().grabFocus()
    except Exception as error:
        print(f"Focus acknowledgement unavailable: {label}: {error}", flush=True)
    while True:
        node.clearCache()
        if node.getState().contains(pyatspi.STATE_FOCUSED):
            return
        if time.monotonic() >= deadline:
            raise AssertionError(f"Installed UI control did not receive focus: {label}")
        time.sleep(0.1)


def activate(label, contains=False):
    started = time.monotonic()
    deadline = started + observation_timeout()
    node = find(label, contains=contains, deadline=deadline, enabled=True)
    focus(node, label, deadline)
    pyatspi.Registry.generateKeyboardEvent(0xff0d, None, pyatspi.KEY_SYM)
    print(f"Submitted installed UI control: {label} ({time.monotonic() - started:.2f}s)", flush=True)


def open_context_menu(label):
    deadline = time.monotonic() + observation_timeout()
    node = find(label, interactive=False, roles=("panel",), deadline=deadline)
    focus(node, label, deadline)
    # ContextMenu and Shift+F10 share the outer node's keyboard contract.
    pyatspi.Registry.generateKeyboardEvent(0xff67, None, pyatspi.KEY_SYM)


def open_menu(label):
    node = find(label)
    deadline = time.monotonic() + observation_timeout()
    focus(node, label, deadline)
    # WebKitGTK exposes the open menu but omits EXPANDED on this trigger.
    # One Return opens it; observing the named menu never toggles it again.
    pyatspi.Registry.generateKeyboardEvent(0xff0d, None, pyatspi.KEY_SYM)
    find(label, interactive=False, roles=("menu",), deadline=deadline)
    print(f"Observed installed menu expansion: {label}", flush=True)


def open_submenu(label):
    node, bounds = locate(label)
    component = node.queryComponent()
    # A submenu trigger has no leaf action. Move off the previous hovered item,
    # then use the menu's focus + Right Arrow keyboard contract.
    pyatspi.Registry.generateMouseEvent(bounds.x + bounds.width // 2, bounds.y + bounds.height // 2, "abs")
    try:
        component.grabFocus()
    except Exception as error:
        print(f"Submenu focus acknowledgement unavailable: {label}: {error}", flush=True)
    deadline = time.monotonic() + observation_timeout()
    keys = 0
    next_key = 0
    focuses = 1
    next_focus = time.monotonic() + 1
    last = None
    while time.monotonic() < deadline:
        try:
            node.clearCache()
            state = node.getState()
            if state.contains(pyatspi.STATE_EXPANDED):
                print(f"Observed installed submenu expansion: {label} ({keys} Right Arrow submissions)", flush=True)
                return
            if not state.contains(pyatspi.STATE_FOCUSED) and focuses < 3 and time.monotonic() >= next_focus:
                focuses += 1
                next_focus = time.monotonic() + 1
                node.queryComponent().grabFocus()
            if state.contains(pyatspi.STATE_FOCUSED) and keys < 3 and time.monotonic() >= next_key:
                # Right Arrow only navigates this submenu; leaf actions remain single clicks.
                keys += 1
                next_key = time.monotonic() + 1
                pyatspi.Registry.generateKeyboardEvent(0xff53, None, pyatspi.KEY_SYM)
        except Exception as error:
            last = error
            try:
                node = find(label, deadline=deadline)
            except Exception as error:
                last = error
        time.sleep(0.1)
    raise AssertionError(f"Installed submenu did not expand: {label}; Right Arrow submissions={keys}; {last}")


def screenshot(path):
    import gi
    gi.require_version("Gdk", "3.0")
    from gi.repository import Gdk
    window = Gdk.get_default_root_window()
    previous = None
    stable = 0
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        Gdk.Display.get_default().sync()
        image = Gdk.pixbuf_get_from_window(window, 0, 0, window.get_width(), window.get_height())
        assert image is not None, "Installed X11 screenshot unavailable"
        pixels = image.get_pixels()
        stable = stable + 1 if pixels == previous else 0
        if stable >= 3:
            image.savev(str(path), "png", [], [])
            return
        previous = pixels
        time.sleep(0.1)
    raise AssertionError("Installed screen did not settle after the UI action")


def capture_problems(output):
    # Read the blocked editor before later API fixtures replace its graph.
    click("Diagnostics", contains=True)
    try:
        find("Current problems", interactive=False, text=True)
        (output / "installed-ui-problems.txt").write_text(tree_text(application()), encoding="utf-8")
        screenshot(output / "installed-ui-problems.png")
    finally:
        pyatspi.Registry.generateKeyboardEvent(0xff1b, None, pyatspi.KEY_SYM)


def main(output):
    from gi.repository import Atspi
    # Keep native queries below the registry's three-second keyboard budget.
    # The default 800ms expires during otherwise successful TCG UI updates.
    Atspi.set_timeout(2000, 15000)
    find("Untitled Graph")
    find("Add apps, devices, effects and more.", interactive=False, text=True)
    open_menu("Add node")
    open_submenu("Processor")
    activate("Compressor")
    find("Compressor", interactive=False, roles=("panel",))
    open_menu("Add node")
    open_submenu("Output")
    activate("Recorder")
    find("Recorder", interactive=False, roles=("panel",))
    open_context_menu("Compressor")
    activate("Connect channels…")
    activate("Destination", contains=True)
    activate("Recorder")
    activate("Connect")
    find("Compressor CH 1 → Recorder CH 1", interactive=False)
    find("Recorder: Record", roles=("toggle button", "button", "push button"), enabled=True)
    tree = tree_text(application())
    assert "Recording error" not in tree, "Recorder must be ready without an error after graph creation"
    (output / "installed-ui-tree.txt").write_text(tree, encoding="utf-8")
    screenshot(output / "installed-ui.png")
    (output / "installed-ui.ready").write_text("ready\n", encoding="utf-8")


if __name__ == "__main__":
    folder = Path(sys.argv[1]).resolve()
    try:
        if sys.argv[2:] == ["--diagnostics"]:
            from gi.repository import Atspi
            Atspi.set_timeout(2000, 15000)
            capture_problems(folder)
        else:
            assert not sys.argv[2:], "Unexpected installed UI arguments"
            main(folder)
    except Exception:
        traceback.print_exc()
        try:
            (folder / "installed-ui-failure.txt").write_text(
                tree_text(pyatspi.Registry.getDesktop(0), diagnostic=True), encoding="utf-8")
            screenshot(folder / "installed-ui-failure.png")
        except Exception:
            traceback.print_exc()
        try:
            # Keep the original observation above. Refresh only the failure diagnostic,
            # so a stale client cache can be distinguished from missing WebKit content.
            desktop = pyatspi.Registry.getDesktop(0)
            desktop.clearCache()
            (folder / "installed-ui-a11y-refreshed.txt").write_text(
                tree_text(desktop, diagnostic=True), encoding="utf-8")
        except Exception:
            traceback.print_exc()
        sys.exit(1)
