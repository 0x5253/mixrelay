//! Exercises public offscreen Cocoa drawing against an actual, disposable AU instance.
//! This is a feasibility probe, not a shipped remote-editor transport or GPU compatibility claim.

#[cfg(target_os = "macos")]
mod macos {
    use std::{
        error::Error,
        path::Path,
        sync::{
            Mutex,
            atomic::{AtomicBool, AtomicU64, Ordering},
        },
        thread,
        time::{Duration, Instant},
    };

    use objc2::{MainThreadMarker, MainThreadOnly, rc::Retained};
    use objc2_app_kit::{
        NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSBitmapImageFileType,
        NSEvent, NSEventMask, NSEventModifierFlags, NSEventType, NSView, NSWindow,
        NSWindowStyleMask,
    };
    use objc2_foundation::{NSDate, NSDefaultRunLoopMode, NSDictionary, NSPoint, NSRect, NSSize};
    use truce_rack_core::{
        AudioBuffer, EventList, Plugin, PluginCore, ProcessContext, ProcessStatus, WindowHandle,
        buffer::BusRange, scanner::PluginScanner,
    };

    pub(super) fn run() -> Result<(), Box<dyn Error>> {
        let main = MainThreadMarker::new().ok_or("probe must run on the main thread")?;
        let app = NSApplication::sharedApplication(main);
        app.setActivationPolicy(NSApplicationActivationPolicy::Prohibited);
        let scanner = truce_rack_au::AuScanner::new();
        let info = scanner
            .scan()?
            .into_iter()
            .find(|info| info.name == "AUBandpass")
            .ok_or("AUBandpass not installed")?;
        let mut plugin = scanner.load(&info)?;
        let layout = plugin
            .supported_layouts()
            .iter()
            .find(|layout| {
                layout.total_input_channels() == 2 && layout.total_output_channels() == 2
            })
            .cloned()
            .ok_or("stereo layout unavailable")?;
        plugin.activate(layout, 48_000.0, 32)?;
        let window = parent_window(main);
        let view = window.contentView().ok_or("missing parent view")?;
        let result = (|| {
            let editor = plugin.editor().ok_or("native editor unavailable")?;
            editor.open(
                WindowHandle::NSView(Retained::as_ptr(&view).cast_mut().cast()),
                1.0,
            )?;
            let (width, height) = editor.size().ok_or("editor size unavailable")?;
            view.setFrameSize(NSSize::new(f64::from(width), f64::from(height)));
            app.finishLaunching();
            // SAFETY: Foundation owns the immutable run-loop mode for the process lifetime.
            let mode = unsafe { NSDefaultRunLoopMode };
            for _ in 0..32 {
                let Some(event) = app.nextEventMatchingMask_untilDate_inMode_dequeue(
                    NSEventMask::Any,
                    Some(&NSDate::distantPast()),
                    mode,
                    true,
                ) else {
                    break;
                };
                app.sendEvent(&event);
            }
            sample_frames(&view, "before")?;
            let before = plugin.parameter_value(0)?;
            drag(&app, &window)?;
            let after = plugin.parameter_value(0)?;
            sample_frames(&view, "after")?;
            eprintln!("native_parameter_before={before} native_parameter_after={after}");
            if (before - after).abs() < 0.001 {
                return Err("native input did not change the parameter".into());
            }
            compare_dsp_progress(&mut plugin, &view)?;
            verify_reopen(&mut plugin, &view)?;
            Ok(())
        })();
        if let Some(editor) = plugin.editor() {
            editor.close();
        }
        plugin.deactivate();
        result
    }

    fn parent_window(main: MainThreadMarker) -> Retained<NSWindow> {
        // SAFETY: main-thread AppKit construction with a finite content rectangle and valid
        // style/backing flags. This retained window owns the parent for the editor's lifetime;
        // it is never ordered onscreen and is dropped only after editor.close().
        unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(main),
                NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(500.0, 200.0)),
                NSWindowStyleMask::Borderless,
                NSBackingStoreType::Buffered,
                false,
            )
        }
    }

    fn drag(app: &NSApplication, window: &NSWindow) -> Result<(), Box<dyn Error>> {
        let content = window.contentView().ok_or("missing parent view")?;
        let target = content
            .hitTest(NSPoint::new(381.0, 162.5))
            .ok_or("no mouse target")?;
        for (kind, x) in [
            (NSEventType::LeftMouseDown, 381.0),
            (NSEventType::LeftMouseDragged, 250.0),
            (NSEventType::LeftMouseUp, 250.0),
        ] {
            let event = NSEvent::mouseEventWithType_location_modifierFlags_timestamp_windowNumber_context_eventNumber_clickCount_pressure(
                kind, NSPoint::new(x, 162.5), NSEventModifierFlags::empty(), 0.0,
                window.windowNumber(), None, 0, 1, 1.0,
            ).ok_or("native mouse event unavailable")?;
            app.postEvent_atStart(&event, false);
        }
        // Queue the whole gesture before dispatch: a control may track synchronously inside
        // mouseDown. Live remoting needs an independent bounded event producer for this reason.
        // SAFETY: Foundation exports this immutable process-lifetime NSString constant.
        let mode = unsafe { NSDefaultRunLoopMode };
        for _ in 0..3 {
            let Some(event) = app.nextEventMatchingMask_untilDate_inMode_dequeue(
                NSEventMask::LeftMouseDown
                    | NSEventMask::LeftMouseDragged
                    | NSEventMask::LeftMouseUp,
                Some(&NSDate::distantPast()),
                mode,
                true,
            ) else {
                break;
            };
            match event.r#type() {
                NSEventType::LeftMouseDown => target.mouseDown(&event),
                NSEventType::LeftMouseDragged => target.mouseDragged(&event),
                NSEventType::LeftMouseUp => target.mouseUp(&event),
                _ => return Err("unexpected mouse event".into()),
            }
        }
        Ok(())
    }

    fn verify_reopen(plugin: &mut impl Plugin<f32>, view: &NSView) -> Result<(), Box<dyn Error>> {
        let before = plugin.parameter_value(0)?;
        for _ in 0..10 {
            let editor = plugin.editor().ok_or("editor disappeared before close")?;
            editor.close();
            if editor.is_open() || view.subviews().count() != 0 {
                return Err("closed editor retained its parent attachment".into());
            }
            editor.open(
                WindowHandle::NSView(std::ptr::from_ref(view).cast_mut().cast()),
                1.0,
            )?;
            if !editor.is_open() || view.subviews().count() != 1 {
                return Err("reopened editor attachment count is not one".into());
            }
        }
        if (plugin.parameter_value(0)? - before).abs() > 0.001 {
            return Err("closing and reopening the editor reset native parameters".into());
        }
        eprintln!("native_editor_reopen_cycles=10 retained_subviews=1");
        Ok(())
    }

    fn compare_dsp_progress(
        plugin: &mut impl Plugin<f32>,
        view: &NSView,
    ) -> Result<(), Box<dyn Error>> {
        let plugin = Mutex::new(plugin);
        let stop = AtomicBool::new(false);
        let blocks = AtomicU64::new(0);
        thread::scope(|scope| {
            // Same instance, 48 kHz / 32 samples and 60 native redraws on both sides.
            // This measures DSP exclusion, not end-to-end latency or a speedup ratio.
            let guard = plugin.lock().map_err(|_| "probe plugin mutex poisoned")?;
            let worker = scope.spawn(|| render_blocks(&plugin, &stop, &blocks));
            let baseline = sample_frames(view, "locked");
            let blocked_count = blocks.load(Ordering::Relaxed);
            drop(guard);
            let initial_count = blocks.load(Ordering::Relaxed);
            let after = sample_frames(view, "unlocked");
            let concurrent_count = blocks.load(Ordering::Relaxed) - initial_count;
            stop.store(true, Ordering::Release);
            let processed = worker.join().map_err(|_| "probe DSP thread panicked")?;
            baseline?;
            after?;
            processed.map_err(|error| -> Box<dyn Error> { error.into() })?;
            eprintln!("locked_dsp_blocks={blocked_count} unlocked_dsp_blocks={concurrent_count}");
            if blocked_count != 0 || concurrent_count == 0 {
                return Err("native redraw/DSP exclusion invariant failed".into());
            }
            Ok(())
        })
    }

    fn render_blocks(
        plugin: &Mutex<&mut impl Plugin<f32>>,
        stop: &AtomicBool,
        blocks: &AtomicU64,
    ) -> Result<(), String> {
        let input = [0.25; 32];
        let mut left = [0.0; 32];
        let mut right = [0.0; 32];
        let buses = [BusRange::new(0, 2)];
        let input_events = EventList::new();
        let mut output_events = EventList::new();
        while !stop.load(Ordering::Acquire) {
            thread::sleep(Duration::from_micros(100));
            let Ok(mut plugin) = plugin.try_lock() else {
                continue;
            };
            let inputs = [&input[..], &input[..]];
            let mut outputs = [&mut left[..], &mut right[..]];
            let mut buffer = AudioBuffer::new(&inputs, &mut outputs, 32, &buses, &buses);
            output_events.clear();
            let mut context = ProcessContext {
                sample_rate: 48_000.0,
                max_block_size: 32,
                transport: None,
                output_events: &mut output_events,
            };
            let status = plugin
                .process(&mut buffer, &input_events, &mut context)
                .map_err(|error| error.to_string())?;
            if status == ProcessStatus::Error
                || left.iter().chain(&right).any(|sample| !sample.is_finite())
            {
                return Err("native DSP returned an error or non-finite PCM".to_owned());
            }
            blocks.fetch_add(1, Ordering::Relaxed);
        }
        Ok(())
    }

    fn sample_frames(view: &NSView, name: &str) -> Result<(), Box<dyn Error>> {
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/validation/plugin-ui-surface");
        std::fs::create_dir_all(&path)?;
        let bounds = view.bounds();
        let bitmap = view
            .bitmapImageRepForCachingDisplayInRect(bounds)
            .ok_or("offscreen bitmap unavailable")?;
        let start = Instant::now();
        for _ in 0..60 {
            view.cacheDisplayInRect_toBitmapImageRep(bounds, &bitmap);
        }
        // SAFETY: an empty, correctly typed properties dictionary requests PNG defaults.
        // The returned retained NSData owns its immutable bytes through the filesystem write.
        let png = unsafe {
            bitmap.representationUsingType_properties(
                NSBitmapImageFileType::PNG,
                &NSDictionary::new(),
            )
        }
        .ok_or("bitmap PNG encoding failed")?;
        std::fs::write(path.join(format!("aubandpass-{name}.png")), png.to_vec())?;
        eprintln!(
            "offscreen_frames=60 elapsed_ms={:.3} png_bytes={} output={}",
            start.elapsed().as_secs_f64() * 1_000.0,
            png.len(),
            path.join(format!("aubandpass-{name}.png")).display()
        );
        Ok(())
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if !std::env::args().any(|arg| arg == "--native-child") {
        return run_bounded_child();
    }
    #[cfg(target_os = "macos")]
    return macos::run();
    #[cfg(not(target_os = "macos"))]
    Err("the Cocoa surface probe requires macOS".into())
}

fn run_bounded_child() -> Result<(), Box<dyn std::error::Error>> {
    let mut child = std::process::Command::new(std::env::current_exe()?)
        .arg("--native-child")
        .spawn()?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => return Err(format!("native surface probe failed: {status}").into()),
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            result => {
                let kill_result = child.kill();
                let wait_result = child.wait();
                kill_result?;
                wait_result?;
                result?;
                return Err("native surface probe exceeded its 20-second deadline".into());
            }
        }
    }
}
