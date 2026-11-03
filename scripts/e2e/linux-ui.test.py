import importlib.util
from pathlib import Path
import sys
import types
import unittest
from tempfile import TemporaryDirectory
from unittest.mock import patch


class Node(list):
    def __init__(self, name, role, children=(), showing=True):
        super().__init__(children)
        self.name = name
        self.role = role
        self.showing = showing

    @property
    def childCount(self):
        return len(self)

    def getRoleName(self):
        return self.role

    def getState(self):
        return types.SimpleNamespace(contains=lambda state: self.showing)

    def clearCache(self):
        pass


class InstalledEditorTest(unittest.TestCase):
    def setUp(self):
        self.desktop = Node("desktop", "desktop")
        registry = types.SimpleNamespace(getDesktop=lambda index: self.desktop)
        self.atspi = types.SimpleNamespace(Registry=registry, STATE_SHOWING=1)
        spec = importlib.util.spec_from_file_location(
            "linux_ui", Path(__file__).with_name("linux-ui.py"))
        self.ui = importlib.util.module_from_spec(spec)
        with patch.dict(sys.modules, pyatspi=self.atspi):
            spec.loader.exec_module(self.ui)
        self.add = Node("Add node", "combo box")
        self.editor = Node("mixrelay-desktop", "application", [
            Node("MixRelay", "frame", [self.add])])
        self.runtime = Node("mixrelay-desktop", "application")

    def test_headless_runtime_does_not_hide_editor_in_either_registration_order(self):
        for apps in ([self.runtime, self.editor], [self.editor, self.runtime]):
            with self.subTest(runtime_first=apps[0] is self.runtime):
                self.ui.application.cache_clear()
                self.desktop[:] = apps
                self.assertIs(self.ui.application(), self.editor)
                self.assertIs(self.ui.find("Add node"), self.add)

    def test_other_apps_and_hidden_editors_cannot_supply_controls(self):
        other = Node("other-product", "application", [Node("MixRelay", "frame", [self.add])])
        hidden = Node("mixrelay-desktop", "application", [
            Node("MixRelay", "frame", [self.add], showing=False)])
        self.desktop[:] = [other, hidden, self.runtime]
        with self.assertRaisesRegex(LookupError, "visible accessibility editor"):
            self.ui.application()

    def test_null_and_defunct_children_do_not_hide_live_controls_or_diagnostics(self):
        class Defunct(Node):
            def getRoleName(self):
                raise RuntimeError("AT-SPI object disappeared")

        class Changing(Node):
            def __getitem__(self, index):
                if index == 1:
                    raise RuntimeError("AT-SPI child disappeared")
                return super().__getitem__(index)

        self.editor.insert(0, None)
        self.editor.append(Changing("panel", "panel", [None, None, Defunct("gone", "button")]))
        self.editor[-1].append(Node("Compressor", "button"))
        self.desktop[:] = [None, Defunct("gone", "application"), self.runtime, self.editor]
        self.assertIs(self.ui.find("Add node"), self.add)
        self.assertEqual(self.ui.find("Compressor").name, "Compressor")
        tree = self.ui.tree_text(self.desktop)
        self.assertIn("combo box Add node", tree)
        self.assertIn("button Compressor", tree)
        self.assertNotIn("gone", tree)
        self.assertEqual(list(self.ui.descendants(None)), [])
        diagnostic = self.ui.tree_text(self.desktop, diagnostic=True)
        self.assertIn("AT-SPI child 0 returned null", diagnostic)
        self.assertIn("AT-SPI child 1 failed: AT-SPI child disappeared", diagnostic)
        self.assertIn("AT-SPI description failed: AT-SPI object disappeared", diagnostic)
        self.assertIn("button Compressor", diagnostic)
        self.assertIn("AT-SPI childCount=0", diagnostic)
        hidden = self.ui.tree_text(Node("Retry connection", "button", showing=False), diagnostic=True)
        self.assertIn("button Retry connection [showing=False]", hidden)

    def test_diagnostics_report_an_unreadable_child_count(self):
        class Unreadable(Node):
            @property
            def childCount(self):
                raise RuntimeError("query timed out")

        tree = self.ui.tree_text(Unreadable("panel", "panel"), diagnostic=True)
        self.assertIn("panel panel", tree)
        self.assertIn("AT-SPI childCount failed: query timed out", tree)

    def test_missing_hidden_or_noninteractive_controls_still_fail(self):
        self.desktop[:] = [self.runtime, self.editor]
        for role, showing in [("combo box", False), ("label", True)]:
            with self.subTest(role=role, showing=showing):
                self.add.role, self.add.showing = role, showing
                with patch.object(self.ui.time, "monotonic", side_effect=[0, 1, 31]), \
                        patch.object(self.ui.time, "sleep"):
                    with self.assertRaisesRegex(AssertionError, "Missing installed UI control"):
                        self.ui.find("Add node")

    def test_a_menu_choice_cannot_stand_in_for_the_rendered_canvas_node(self):
        item = Node("Compressor", "menu item")
        card = Node("Compressor", "panel")
        self.editor[0][:] = [item, card]
        self.desktop[:] = [self.editor]
        self.assertIs(self.ui.find("Compressor", interactive=False, roles=("panel",)), card)

    def test_recorder_readiness_observes_enabled_state_and_fails_if_it_never_arrives(self):
        self.desktop[:] = [self.editor]
        self.atspi.STATE_ENABLED = 8
        record = Node("Recorder: Record", "toggle button")
        self.editor[0].append(record)
        elapsed = 0
        ready = False
        record.getState = lambda: types.SimpleNamespace(contains=lambda flag:
            True if flag == self.atspi.STATE_SHOWING else ready)
        def advance(seconds):
            nonlocal elapsed, ready
            elapsed += seconds
            ready = True
        with patch.object(self.ui.time, "monotonic", side_effect=lambda: elapsed), \
                patch.object(self.ui.time, "sleep", side_effect=advance):
            self.assertIs(self.ui.find("Recorder: Record", roles=("toggle button",), enabled=True), record)
        self.assertGreater(elapsed, 0)
        ready = False
        with patch.object(self.ui.time, "monotonic", side_effect=[0, 1, 31]), \
                patch.object(self.ui.time, "sleep"):
            with self.assertRaisesRegex(AssertionError, "Missing installed UI control"):
                self.ui.find("Recorder: Record", roles=("toggle button",), enabled=True)

    def test_empty_canvas_paragraph_is_read_through_its_text_interface(self):
        paragraph = Node("", "paragraph")
        paragraph.queryText = lambda: types.SimpleNamespace(getText=lambda start, end: "Empty canvas")
        self.editor[0][:] = [paragraph]
        self.desktop[:] = [self.editor]
        self.assertIs(self.ui.find("Empty canvas", interactive=False, text=True), paragraph)

    def test_activation_posts_one_pointer_click_without_an_uncertain_accessibility_action(self):
        self.desktop[:] = [self.editor]
        self.add.queryComponent = lambda: types.SimpleNamespace(
            getExtents=lambda coords: types.SimpleNamespace(x=10, y=20, width=30, height=40))
        self.atspi.DESKTOP_COORDS = 0
        events = []
        self.atspi.Registry.generateMouseEvent = lambda *args: events.append(args)
        def uncertain_action():
            raise RuntimeError("AT-SPI action timed out after dispatch")
        self.add.queryAction = uncertain_action
        self.ui.click("Add node")
        self.assertEqual(events, [(25, 40, "abs"), (25, 40, "b1c")])
        events.clear()
        self.ui.click("Add node", right=True)
        self.assertEqual(events, [(25, 40, "abs"), (25, 40, "b3c")])

    def test_unrelated_controls_do_not_require_role_or_state_round_trips(self):
        reads = []
        class Busy(Node):
            def getRoleName(self):
                reads.append("role")
                return super().getRoleName()

            def getState(self):
                reads.append("state")
                return super().getState()

        self.editor[0].insert(0, Busy("Unrelated", "panel"))
        self.desktop[:] = [self.editor]
        self.assertIs(self.ui.find("Add node"), self.add)
        self.assertEqual(reads, [])

    def test_leaf_waits_for_enabled_and_observed_focus_before_one_return(self):
        self.desktop[:] = [self.editor]
        item = Node("Recorder", "menu item")
        self.editor[0].extend([Node("Recorder", "panel"), item])
        self.atspi.STATE_ENABLED = 4
        self.atspi.STATE_FOCUSED = 2
        self.atspi.KEY_SYM = 3
        elapsed = 0
        requested = False
        events = []
        def pause(seconds):
            nonlocal elapsed
            elapsed += seconds
        def grab():
            nonlocal requested
            self.assertGreaterEqual(elapsed, 0.1)
            events.append("focus")
            requested = True
            return True
        item.queryComponent = lambda: types.SimpleNamespace(grabFocus=grab)
        item.getState = lambda: types.SimpleNamespace(contains=lambda state:
            elapsed >= 0.1 if state == self.atspi.STATE_ENABLED else
            requested and elapsed >= 0.2 if state == self.atspi.STATE_FOCUSED else True)
        self.atspi.Registry.generateKeyboardEvent = lambda *args: events.append(args)
        self.atspi.Registry.generateMouseEvent = lambda *args: self.fail("Leaf must use observed focus and Return")
        with patch.object(self.ui.time, "monotonic", side_effect=lambda: elapsed), \
                patch.object(self.ui.time, "sleep", side_effect=pause):
            self.ui.activate("Recorder")
        self.assertEqual(events, ["focus", (0xff0d, None, 3)])
        self.assertGreaterEqual(elapsed, 0.2)

    def test_unobserved_focus_never_submits_a_leaf_action(self):
        self.desktop[:] = [self.editor]
        self.atspi.STATE_FOCUSED = 2
        self.atspi.STATE_ENABLED = 4
        self.atspi.KEY_SYM = 3
        self.add.queryComponent = lambda: types.SimpleNamespace(grabFocus=lambda: True)
        self.add.getState = lambda: types.SimpleNamespace(contains=lambda state: state != self.atspi.STATE_FOCUSED)
        events = []
        self.atspi.Registry.generateKeyboardEvent = lambda *args: events.append(args)
        elapsed = 0
        def pause(seconds):
            nonlocal elapsed
            elapsed += 31
        with patch.object(self.ui.time, "monotonic", side_effect=lambda: elapsed), \
                patch.object(self.ui.time, "sleep", side_effect=pause):
            with self.assertRaisesRegex(AssertionError, "did not receive focus"):
                self.ui.activate("Add node")
        self.assertEqual(events, [])

    def test_lost_focus_acknowledgement_requires_observed_focus_without_replaying(self):
        self.desktop[:] = [self.editor]
        self.atspi.STATE_FOCUSED = 2
        self.atspi.STATE_ENABLED = 4
        self.atspi.KEY_SYM = 3
        for applied in [True, False]:
            with self.subTest(applied=applied):
                events = []
                elapsed = 0
                observed = {"focused": False}
                def grab():
                    events.append("focus")
                    raise TimeoutError("AT-SPI focus reply was lost")
                def pause(seconds):
                    nonlocal elapsed
                    elapsed += 31
                self.add.queryComponent = lambda: types.SimpleNamespace(grabFocus=grab)
                self.add.clearCache = lambda: observed.update(focused=applied)
                self.add.getState = lambda: types.SimpleNamespace(contains=lambda state:
                    observed["focused"] if state == self.atspi.STATE_FOCUSED else True)
                self.atspi.Registry.generateKeyboardEvent = lambda *args: events.append(args)
                with patch.object(self.ui.time, "monotonic", side_effect=lambda: elapsed), \
                        patch.object(self.ui.time, "sleep", side_effect=pause):
                    if applied:
                        self.ui.activate("Add node")
                        self.assertEqual(events, ["focus", (0xff0d, None, 3)])
                    else:
                        with self.assertRaisesRegex(AssertionError, "did not receive focus"):
                            self.ui.activate("Add node")
                        self.assertEqual(events, ["focus"])

    def test_node_context_menu_uses_the_outer_canvas_panel(self):
        self.desktop[:] = [self.editor]
        panel = Node("Compressor", "panel")
        self.editor[0].extend([Node("Compressor", "menu item"), panel])
        self.atspi.STATE_FOCUSED = 2
        self.atspi.KEY_SYM = 3
        events = []
        panel.queryComponent = lambda: types.SimpleNamespace(grabFocus=lambda: events.append("panel focus"))
        self.atspi.Registry.generateKeyboardEvent = lambda *args: events.append(args)
        self.ui.open_context_menu("Compressor")
        self.assertEqual(events, ["panel focus", (0xff67, None, 3)])

    def test_bounds_queries_can_recover_before_the_single_pointer_action(self):
        self.desktop[:] = [self.editor]
        self.atspi.DESKTOP_COORDS = 0
        reads = []
        def bounds(coords):
            reads.append(coords)
            if len(reads) == 1:
                raise RuntimeError("Read-only AT-SPI query timed out")
            return types.SimpleNamespace(x=10, y=20, width=30, height=40)
        self.add.queryComponent = lambda: types.SimpleNamespace(getExtents=bounds)
        events = []
        self.atspi.Registry.generateMouseEvent = lambda *args: events.append(args)
        with patch.object(self.ui.time, "sleep"):
            self.ui.click("Add node")
        self.assertEqual(len(reads), 2)
        self.assertEqual(events, [(25, 40, "abs"), (25, 40, "b1c")])

    def test_submenu_receives_focus_and_right_arrow_instead_of_a_leaf_action(self):
        self.desktop[:] = [self.editor]
        self.atspi.DESKTOP_COORDS = 0
        self.atspi.STATE_FOCUSED = 2
        self.atspi.STATE_EXPANDED = 4
        self.atspi.KEY_SYM = 3
        events = []
        self.atspi.Registry.generateMouseEvent = lambda *args: events.append(args)
        expanded = False
        def key(*args):
            nonlocal expanded
            events.append(args)
            expanded = True
        self.atspi.Registry.generateKeyboardEvent = key
        self.add.getState = lambda: types.SimpleNamespace(contains=lambda state:
            expanded if state == self.atspi.STATE_EXPANDED else True)
        component = types.SimpleNamespace(
            getExtents=lambda coords: types.SimpleNamespace(x=10, y=20, width=30, height=40),
            grabFocus=lambda: events.append("focus") or True)
        self.add.queryComponent = lambda: component
        self.ui.open_submenu("Add node")
        self.assertEqual(events, [(25, 40, "abs"), "focus", (0xff53, None, 3)])
        events.clear()
        component.grabFocus = lambda: False
        self.add.getState = lambda: types.SimpleNamespace(contains=lambda state: state == self.atspi.STATE_SHOWING)
        elapsed = 0
        def pause(seconds):
            nonlocal elapsed
            elapsed += 31
        with patch.object(self.ui.time, "monotonic", side_effect=lambda: elapsed), \
                patch.object(self.ui.time, "sleep", side_effect=pause):
            with self.assertRaisesRegex(AssertionError, "did not expand"):
                self.ui.open_submenu("Add node")
        self.assertEqual(events, [(25, 40, "abs")])

    def test_menu_trigger_requires_focus_and_a_visible_menu_without_a_trigger_expanded_flag(self):
        self.desktop[:] = [self.editor]
        self.atspi.STATE_FOCUSED = 2
        self.atspi.STATE_EXPANDED = 4
        self.atspi.KEY_SYM = 3
        state = {"focused": False}
        observed = state.copy()
        menu = Node("Add node", "menu", [Node("Processor", "menu item")])
        keys = []
        self.add.clearCache = lambda: observed.update(state)
        self.add.getState = lambda: types.SimpleNamespace(contains=lambda flag:
            self.add.showing if flag == self.atspi.STATE_SHOWING else
            observed["focused"] if flag == self.atspi.STATE_FOCUSED else False)
        self.add.queryComponent = lambda: types.SimpleNamespace(
            grabFocus=lambda: state.update(focused=True))
        def submit(*args):
            self.assertTrue(observed["focused"], "The trigger must own keyboard focus")
            keys.append(args)
            self.editor[0].append(menu)
        self.atspi.Registry.generateKeyboardEvent = submit
        elapsed = 0
        def pause(seconds):
            nonlocal elapsed
            elapsed += 1
        with patch.object(self.ui.time, "monotonic", side_effect=lambda: elapsed), \
                patch.object(self.ui.time, "sleep", side_effect=pause):
            self.ui.open_menu("Add node")
        self.assertIs(self.ui.find("Add node", interactive=False, roles=("menu",)), menu)
        self.assertEqual(keys, [(0xff0d, None, 3)])
        self.editor[0].remove(menu)

        state.update(focused=False)
        observed.update(state)
        keys.clear()
        elapsed = 0
        self.add.queryComponent = lambda: types.SimpleNamespace(grabFocus=lambda: False)
        with patch.object(self.ui.time, "monotonic", side_effect=lambda: elapsed), \
                patch.object(self.ui.time, "sleep", side_effect=pause):
            with self.assertRaisesRegex(AssertionError, "did not receive focus"):
                self.ui.open_menu("Add node")
        self.assertEqual(keys, [])

        elapsed = 0
        self.add.queryComponent = lambda: types.SimpleNamespace(
            grabFocus=lambda: state.update(focused=True))
        self.atspi.Registry.generateKeyboardEvent = lambda *args: keys.append(args)
        with patch.object(self.ui.time, "monotonic", side_effect=lambda: elapsed), \
                patch.object(self.ui.time, "sleep", side_effect=pause):
            with self.assertRaisesRegex(AssertionError, "Missing installed UI control"):
                self.ui.open_menu("Add node")
        self.assertEqual(keys, [(0xff0d, None, 3)], "An unacknowledged opening must not toggle the menu again")

    def test_problem_capture_reads_the_dialog_and_always_dismisses_it_without_a_write(self):
        self.atspi.KEY_SYM = 3
        events = []
        self.atspi.Registry.generateKeyboardEvent = lambda *args: events.append(args)
        with TemporaryDirectory() as directory:
            output = Path(directory)
            with patch.object(self.ui, "click") as click, \
                    patch.object(self.ui, "find") as find, \
                    patch.object(self.ui, "application", return_value=self.editor), \
                    patch.object(self.ui, "tree_text", return_value="graph_apply_timeout"), \
                    patch.object(self.ui, "screenshot"):
                self.ui.capture_problems(output)
                click.assert_called_once_with("Diagnostics", contains=True)
                find.assert_called_once_with("Current problems", interactive=False, text=True)
                self.assertEqual((output / "installed-ui-problems.txt").read_text(), "graph_apply_timeout")
                find.side_effect = RuntimeError("dialog unavailable")
                with self.assertRaisesRegex(RuntimeError, "dialog unavailable"):
                    self.ui.capture_problems(output)
        self.assertEqual(events, [(0xff1b, None, 3), (0xff1b, None, 3)])

    def test_collapsed_submenu_must_acknowledge_navigation_before_a_leaf_can_be_selected(self):
        self.desktop[:] = [self.editor]
        self.atspi.DESKTOP_COORDS = 0
        self.atspi.STATE_FOCUSED = 2
        self.atspi.STATE_EXPANDED = 4
        self.atspi.KEY_SYM = 3
        state = {"expanded": False, "keys": 0, "fresh": 0}
        self.add.clearCache = lambda: state.update(fresh=state["fresh"] + 1)
        self.add.queryComponent = lambda: types.SimpleNamespace(
            getExtents=lambda coords: types.SimpleNamespace(x=10, y=20, width=30, height=40),
            grabFocus=lambda: True)
        self.add.getState = lambda: types.SimpleNamespace(contains=lambda flag:
            state["expanded"] if flag == self.atspi.STATE_EXPANDED else True)
        self.atspi.Registry.generateMouseEvent = lambda *args: None
        def key(*args):
            self.assertEqual(args, (0xff53, None, 3))
            state["keys"] += 1
            state["expanded"] = state["keys"] == 2
        self.atspi.Registry.generateKeyboardEvent = key
        elapsed = 0
        def pause(seconds):
            nonlocal elapsed
            elapsed += 0.5
        with patch.object(self.ui.time, "monotonic", side_effect=lambda: elapsed), \
                patch.object(self.ui.time, "sleep", side_effect=pause):
            self.ui.open_submenu("Add node")
        self.assertTrue(state["expanded"], "Posting a key is not proof that the submenu opened")
        self.assertEqual(state["keys"], 2)
        self.assertGreater(state["fresh"], 0)
        state.update(expanded=False, keys=0)
        elapsed = 0
        self.atspi.Registry.generateKeyboardEvent = lambda *args: state.update(keys=state["keys"] + 1)
        with patch.object(self.ui.time, "monotonic", side_effect=lambda: elapsed), \
                patch.object(self.ui.time, "sleep", side_effect=pause):
            with self.assertRaisesRegex(AssertionError, "did not expand"):
                self.ui.open_submenu("Add node")
        self.assertEqual(state["keys"], 3)
        state.update(expanded=False, keys=0)
        elapsed = 0
        def delivered_without_acknowledgement(*args):
            state.update(expanded=True, keys=state["keys"] + 1)
            raise RuntimeError("Keyboard acknowledgement was lost after delivery")
        self.atspi.Registry.generateKeyboardEvent = delivered_without_acknowledgement
        with patch.object(self.ui.time, "monotonic", side_effect=lambda: elapsed), \
                patch.object(self.ui.time, "sleep", side_effect=pause):
            self.ui.open_submenu("Add node")
        self.assertEqual(state["keys"], 1)


if __name__ == "__main__":
    unittest.main()
