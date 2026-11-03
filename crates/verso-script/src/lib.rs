//! Lua scripting for Verso.
//!
//! A script is Lua 5.4 code that may define two functions:
//!
//! ```lua
//! function frame(t)   -- every frame; t.time, t.dt, t.beats, t.bpm,
//!                     -- t.audio.level|bass|mid|high|beat
//!   verso.set("/layer/1/opacity", 0.5 + 0.5 * math.sin(t.beats * math.pi))
//! end
//!
//! function beat(n)    -- on every beat of the composition tempo
//!   verso.trigger("/column/" .. (n % 4 + 1) .. "/launch")
//! end
//! ```
//!
//! The `verso` table addresses the composition with the OSC address space
//! (`verso_core::control`), values in 0..1:
//!
//! - `verso.set(address, value)` sets a parameter or fires a trigger
//! - `verso.trigger(address)` same as `set(address, 1)`
//! - `verso.get(address)` current value in 0..1, or `nil`
//! - `verso.layers()`, `verso.columns()` counts
//! - `verso.log(...)` prints to the script console
//!
//! Scripts run sandboxed: no `io`, `os`, `require` or file loading, and each
//! call is limited in instructions so an endless loop cannot freeze a show.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use mlua::{HookTriggers, Lua, LuaOptions, StdLib, Table, Value, VmState};
use verso_core::control::{Action, Param, Range};
use verso_core::{Command, Composition};

/// Instructions one script may run per call, checked every 1000.
const INSTRUCTION_BUDGET: u64 = 5_000_000;
const HOOK_STEP: u32 = 1000;
/// Memory one script may hold.
const MEMORY_LIMIT: usize = 64 * 1024 * 1024;
const TOO_LONG: &str = "script took too long (endless loop?)";
/// Console lines kept per script.
const LOG_LINES: usize = 200;

/// Per-frame values passed to `frame(t)`.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct FrameInfo {
    pub time: f64,
    pub dt: f64,
    pub beats: f64,
    pub bpm: f32,
    /// level, bass, mid, high, beat.
    pub audio: [f32; 5],
}

struct Script {
    name: String,
    source: String,
    lua: Lua,
    /// Instructions run in the current call, in steps of [`HOOK_STEP`].
    steps: Rc<Cell<u64>>,
    error: Option<String>,
    log: Vec<String>,
}

/// Runs a set of named scripts against a composition.
#[derive(Default)]
pub struct ScriptHost {
    scripts: Vec<Script>,
    /// Last whole beat seen, to call `beat(n)` once per beat.
    last_beat: Option<i64>,
}

fn sandbox() -> mlua::Result<(Lua, Rc<Cell<u64>>)> {
    let lua = Lua::new_with(
        StdLib::MATH | StdLib::STRING | StdLib::TABLE | StdLib::UTF8,
        LuaOptions::default(),
    )?;
    lua.set_memory_limit(MEMORY_LIMIT)?;
    // The base library is always loaded; remove what reaches the file system.
    let globals = lua.globals();
    for name in ["dofile", "loadfile", "load", "require", "collectgarbage"] {
        globals.raw_set(name, Value::Nil)?;
    }
    let steps = Rc::new(Cell::new(0u64));
    let over_budget = {
        let steps = steps.clone();
        move || steps.get().saturating_mul(HOOK_STEP as u64) > INSTRUCTION_BUDGET
    };
    // `pcall` and `xpcall` would catch the error raised when the budget runs
    // out and let an endless loop continue: once over budget, they raise it
    // again outside the protected call.
    for name in ["pcall", "xpcall"] {
        let original: mlua::Function = globals.get(name)?;
        let over_budget = over_budget.clone();
        let wrapped = lua.create_function(move |_, args: mlua::MultiValue| {
            let results = original.call::<mlua::MultiValue>(args)?;
            if over_budget() {
                return Err(mlua::Error::runtime(TOO_LONG));
            }
            Ok(results)
        })?;
        globals.raw_set(name, wrapped)?;
    }
    let counter = steps.clone();
    lua.set_hook(
        HookTriggers::new().every_nth_instruction(HOOK_STEP),
        move |_, _| {
            counter.set(counter.get().saturating_add(1));
            if over_budget() {
                return Err(mlua::Error::runtime(TOO_LONG));
            }
            Ok(VmState::Continue)
        },
    )?;
    Ok((lua, steps))
}

/// Converts a value from a script: numbers as-is, booleans to 0 or 1.
fn unit(value: Value) -> mlua::Result<f32> {
    match value {
        Value::Nil => Ok(1.0),
        Value::Boolean(b) => Ok(if b { 1.0 } else { 0.0 }),
        Value::Integer(i) => Ok(i as f32),
        Value::Number(n) => Ok(n as f32),
        other => Err(mlua::Error::runtime(format!(
            "expected a number, got {}",
            other.type_name()
        ))),
    }
}

impl ScriptHost {
    /// Loads or reloads the script `name`. A script that fails to compile is
    /// kept with its error so the console can show it.
    pub fn load(&mut self, name: &str, source: &str) {
        let position = self.scripts.iter().position(|s| s.name == name);
        if let Some(i) = position
            && self.scripts[i].source == source
        {
            return;
        }
        let (lua, steps, error) = match sandbox() {
            Ok((lua, steps)) => {
                steps.set(0);
                let error = lua
                    .load(source)
                    .set_name(name)
                    .exec()
                    .err()
                    .map(|e| e.to_string());
                (lua, steps, error)
            }
            Err(e) => {
                let lua = Lua::new();
                (lua, Rc::new(Cell::new(0u64)), Some(e.to_string()))
            }
        };
        let script = Script {
            name: name.to_owned(),
            source: source.to_owned(),
            lua,
            steps,
            error,
            log: Vec::new(),
        };
        match position {
            Some(i) => self.scripts[i] = script,
            None => self.scripts.push(script),
        }
    }

    /// Drops scripts whose names are not in `names`.
    pub fn retain(&mut self, names: &[&str]) {
        self.scripts.retain(|s| names.contains(&s.name.as_str()));
    }

    pub fn error(&self, name: &str) -> Option<&str> {
        self.scripts
            .iter()
            .find(|s| s.name == name)
            .and_then(|s| s.error.as_deref())
    }

    pub fn log(&self, name: &str) -> &[String] {
        self.scripts
            .iter()
            .find(|s| s.name == name)
            .map(|s| s.log.as_slice())
            .unwrap_or_default()
    }

    /// Runs `frame(t)` and, when a beat was crossed, `beat(n)` of every
    /// script without an error. Returns the commands they issued. A script
    /// that raises an error stops until it is reloaded.
    ///
    /// `range` gives the range and value of parameters without a built-in
    /// range (effect parameters).
    pub fn run(
        &mut self,
        comp: &Composition,
        info: &FrameInfo,
        range: &dyn Fn(&Param) -> Option<(Range, f32)>,
    ) -> Vec<Command> {
        let beat = info.beats.floor() as i64;
        let new_beat = match self.last_beat {
            Some(last) if beat > last => Some(beat),
            None => None,
            _ => None,
        };
        self.last_beat = Some(beat);

        let commands = RefCell::new(Vec::new());
        for script in self.scripts.iter_mut().filter(|s| s.error.is_none()) {
            script.steps.set(0);
            let log = RefCell::new(Vec::new());
            let result = run_script(script, comp, info, new_beat, range, &commands, &log);
            script.log.extend(log.into_inner());
            if script.log.len() > LOG_LINES {
                script.log.drain(..script.log.len() - LOG_LINES);
            }
            if let Err(e) = result {
                script.error = Some(e.to_string());
            }
        }
        commands.into_inner()
    }
}

fn parse(address: &str) -> mlua::Result<Action> {
    Action::parse(address).ok_or_else(|| mlua::Error::runtime(format!("unknown address {address}")))
}

fn param_range(
    param: &Param,
    comp: &Composition,
    range: &dyn Fn(&Param) -> Option<(Range, f32)>,
) -> Option<(Range, f32)> {
    match param.builtin_range() {
        Some(r) => Some((r, param.get(comp)?)),
        None => range(param),
    }
}

fn run_script(
    script: &Script,
    comp: &Composition,
    info: &FrameInfo,
    new_beat: Option<i64>,
    range: &dyn Fn(&Param) -> Option<(Range, f32)>,
    commands: &RefCell<Vec<Command>>,
    log: &RefCell<Vec<String>>,
) -> mlua::Result<()> {
    let lua = &script.lua;
    let frame: Option<mlua::Function> = lua.globals().get("frame")?;
    let beat: Option<mlua::Function> = lua.globals().get("beat")?;
    if frame.is_none() && (beat.is_none() || new_beat.is_none()) {
        return Ok(());
    }
    // Defined outside the scope so the scoped functions can borrow them.
    let set = |address: String, value: f32| -> mlua::Result<()> {
        let action = parse(&address)?;
        let r = match &action {
            Action::Set(param) => param_range(param, comp, range).map(|(r, _)| r),
            _ => None,
        };
        if let Some(command) = action.command(value, r) {
            commands.borrow_mut().push(command);
        }
        Ok(())
    };
    let set = &set;
    lua.scope(|scope| {
        let verso = lua.create_table()?;
        verso.set(
            "set",
            scope.create_function(move |_, (address, value): (String, Value)| {
                set(address, unit(value)?)
            })?,
        )?;
        verso.set(
            "trigger",
            scope.create_function(move |_, address: String| set(address, 1.0))?,
        )?;
        verso.set(
            "get",
            scope.create_function(|_, address: String| {
                Ok(match parse(&address)? {
                    Action::Set(param) => {
                        param_range(&param, comp, range).map(|(r, v)| r.to_unit(v))
                    }
                    _ => None,
                })
            })?,
        )?;
        verso.set(
            "layers",
            scope.create_function(|_, ()| Ok(comp.layers.len()))?,
        )?;
        verso.set(
            "columns",
            scope.create_function(|_, ()| Ok(comp.columns.len()))?,
        )?;
        verso.set(
            "log",
            scope.create_function(|lua, values: mlua::Variadic<Value>| {
                let parts: Vec<String> = values
                    .into_iter()
                    .map(|v| {
                        lua.coerce_string(v.clone())
                            .ok()
                            .flatten()
                            .map(|s| s.to_string_lossy())
                            .unwrap_or_else(|| format!("{v:?}"))
                    })
                    .collect();
                log.borrow_mut().push(parts.join(" "));
                Ok(())
            })?,
        )?;
        lua.globals().set("verso", verso)?;

        if let Some(frame) = &frame {
            let t: Table = lua.create_table()?;
            t.set("time", info.time)?;
            t.set("dt", info.dt)?;
            t.set("beats", info.beats)?;
            t.set("bpm", info.bpm)?;
            let audio = lua.create_table()?;
            for (name, value) in ["level", "bass", "mid", "high", "beat"]
                .iter()
                .zip(info.audio)
            {
                audio.set(*name, value)?;
            }
            t.set("audio", audio)?;
            frame.call::<()>(t)?;
        }
        if let (Some(beat), Some(n)) = (&beat, new_beat) {
            beat.call::<()>(n)?;
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use verso_core::EffectTarget;

    fn run(host: &mut ScriptHost, comp: &Composition, beats: f64) -> Vec<Command> {
        let info = FrameInfo {
            beats,
            bpm: comp.bpm,
            ..Default::default()
        };
        host.run(comp, &info, &|_| None)
    }

    #[test]
    fn frame_sets_parameters() {
        let comp = Composition::demo();
        let mut host = ScriptHost::default();
        host.load(
            "fade",
            r#"function frame(t) verso.set("/layer/2/opacity", t.beats / 4) end"#,
        );
        assert_eq!(
            run(&mut host, &comp, 2.0),
            [Command::SetLayerOpacity {
                layer: 1,
                opacity: 0.5
            }]
        );
    }

    #[test]
    fn beat_fires_once_per_beat() {
        let comp = Composition::demo();
        let mut host = ScriptHost::default();
        host.load(
            "chase",
            r#"function beat(n) verso.trigger("/column/" .. (n % 3 + 1) .. "/launch") end"#,
        );
        assert!(
            run(&mut host, &comp, 0.5).is_empty(),
            "no beat on the first frame"
        );
        assert!(run(&mut host, &comp, 0.9).is_empty());
        // Beat 1 launches column 2, which is index 1.
        assert_eq!(run(&mut host, &comp, 1.1), [Command::TriggerColumn(1)]);
        assert!(run(&mut host, &comp, 1.6).is_empty());
        assert_eq!(run(&mut host, &comp, 2.0), [Command::TriggerColumn(2)]);
    }

    #[test]
    fn get_reads_values_and_state_persists() {
        let mut comp = Composition::demo();
        comp.bpm = 260.0;
        let mut host = ScriptHost::default();
        host.load(
            "state",
            r#"
            count = 0
            function frame(t)
                count = count + 1
                verso.log("bpm", verso.get("/bpm"), "count", count, "layers", verso.layers())
            end
            "#,
        );
        run(&mut host, &comp, 0.0);
        run(&mut host, &comp, 0.0);
        assert_eq!(
            host.log("state"),
            ["bpm 0.5 count 1 layers 3", "bpm 0.5 count 2 layers 3"]
        );
    }

    #[test]
    fn errors_stop_the_script_and_are_reported() {
        let comp = Composition::demo();
        let mut host = ScriptHost::default();
        host.load("syntax", "function frame(t) end end");
        assert!(host.error("syntax").is_some());

        host.load("runtime", r#"function frame(t) verso.set("/nope", 1) end"#);
        assert!(run(&mut host, &comp, 0.0).is_empty());
        assert!(host.error("runtime").unwrap().contains("/nope"));

        // Reloading with new source clears the error.
        host.load(
            "runtime",
            r#"function frame(t) verso.trigger("/clear") end"#,
        );
        assert_eq!(run(&mut host, &comp, 0.0), [Command::ClearAll]);
    }

    #[test]
    fn endless_loops_are_stopped() {
        let comp = Composition::demo();
        let mut host = ScriptHost::default();
        host.load("loop", "function frame(t) while true do end end");
        run(&mut host, &comp, 0.0);
        assert!(host.error("loop").unwrap().contains("too long"));
    }

    #[test]
    fn pcall_cannot_catch_the_instruction_budget() {
        let comp = Composition::demo();
        let mut host = ScriptHost::default();
        host.load(
            "escape",
            r#"function frame(t)
                while true do
                    pcall(function() while true do end end)
                    xpcall(function() while true do end end, function(e) return e end)
                end
            end"#,
        );
        let start = std::time::Instant::now();
        run(&mut host, &comp, 0.0);
        assert!(start.elapsed() < std::time::Duration::from_secs(5));
        assert!(host.error("escape").unwrap().contains("too long"));
    }

    #[test]
    fn pcall_still_catches_ordinary_errors() {
        let comp = Composition::demo();
        let mut host = ScriptHost::default();
        host.load(
            "safe",
            r#"function frame(t)
                local ok, e = pcall(error, "boom")
                verso.log(tostring(ok), e)
            end"#,
        );
        run(&mut host, &comp, 0.0);
        assert_eq!(host.error("safe"), None);
        assert_eq!(host.log("safe"), ["false boom"]);
    }

    #[test]
    fn memory_is_limited() {
        let comp = Composition::demo();
        let mut host = ScriptHost::default();
        host.load(
            "bomb",
            r#"function frame(t) local s = "x" for i = 1, 40 do s = s .. s end end"#,
        );
        run(&mut host, &comp, 0.0);
        assert!(
            host.error("bomb").unwrap().contains("memory"),
            "{:?}",
            host.error("bomb")
        );
    }

    #[test]
    fn sandbox_has_no_file_or_os_access() {
        let comp = Composition::demo();
        let mut host = ScriptHost::default();
        host.load(
            "probe",
            r#"function frame(t) verso.log(type(io), type(os), type(require), type(load), type(math)) end"#,
        );
        run(&mut host, &comp, 0.0);
        assert_eq!(host.log("probe"), ["nil nil nil nil table"]);
    }

    #[test]
    fn effect_params_use_the_supplied_range() {
        let mut comp = Composition::demo();
        comp.apply(Command::EditEffects {
            target: EffectTarget::Layer(0),
            edit: verso_core::EffectEdit::Add(verso_core::Effect::new("builtin:blur", "Blur")),
        })
        .unwrap();
        let mut host = ScriptHost::default();
        host.load(
            "fx",
            r#"function frame(t) verso.set("/layer/1/effect/1/radius", 0.5) end"#,
        );
        let range = Range {
            min: 0.0,
            max: 2.0,
            toggle: false,
        };
        let commands = host.run(&comp, &FrameInfo::default(), &|_| Some((range, 0.0)));
        assert_eq!(
            commands,
            [Command::EditEffects {
                target: EffectTarget::Layer(0),
                edit: verso_core::EffectEdit::SetParam {
                    index: 0,
                    name: "radius".into(),
                    value: verso_core::ParamValue::Float(1.0),
                },
            }]
        );
    }
}

#[cfg(test)]
mod example_tests {
    use super::*;

    #[test]
    fn repository_example_runs() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/beat-chase.lua");
        let source = std::fs::read_to_string(path).unwrap();
        let comp = Composition::demo();
        let mut host = ScriptHost::default();
        host.load("example", &source);
        let mut info = FrameInfo {
            beats: 3.9,
            ..Default::default()
        };
        host.run(&comp, &info, &|_| None);
        info.beats = 4.1;
        info.audio[1] = 1.0;
        let commands = host.run(&comp, &info, &|_| None);
        assert_eq!(host.error("example"), None);
        assert!(
            commands.contains(&Command::SetMasterOpacity(1.0)),
            "{commands:?}"
        );
        assert!(
            commands.contains(&Command::TriggerColumn(1)),
            "{commands:?}"
        );
    }
}
