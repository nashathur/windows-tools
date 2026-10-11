//! Bus widget dockctl commands and their asynchronous UI replies.

use crate::{data, picker, ttqueue, worker};
use serde_json::{json, Value};
use std::collections::HashSet;
use taskbar_dock::block::{CommandError, CommandReply, ReplyToken};
use windows_sys::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

/// Message posted when an asynchronous command has finished.
pub(crate) const WM_COMMAND_DONE: u32 = WM_APP + 4;

/// A completed command result waiting for the Block UI thread.
pub(crate) struct Posted {
    token: ReplyToken,
    result: PostedResult,
}

/// The result category determines whether finishing also updates the selected config.
enum PostedResult {
    Json(Result<Value, CommandError>),
    Choose(Result<Chosen, CommandError>),
}

/// A resolved line choice ready to save on the UI thread.
struct Chosen {
    stop_name: String,
    stop_code: String,
    line: data::LineOpt,
}

/// Parsed arguments for the `choose` command.
#[derive(Clone, Debug, PartialEq, Eq)]
struct ChooseArgs {
    code: String,
    line: String,
    agency: Option<String>,
    to: Option<String>,
}

/// A `choose` resolution can fail as usage or as an operational error.
#[derive(Clone, Debug, PartialEq, Eq)]
enum ChooseFailure {
    Usage(String),
    Error(String),
}

/// Lists the bus widget's dockctl commands and their usage lines.
pub(crate) fn commands() -> Vec<(&'static str, &'static str)> {
    vec![
        ("now", "now: line, stop and next departures (live or scheduled)"),
        ("stops", "stops <query>: stops whose name matches (or a stop code)"),
        ("lines", "lines <stop_code>: lines and directions at a stop"),
        ("choose", "choose <stop_code> <line> [agency] [--to <n|text>]: show this line at this stop"),
    ]
}

/// Starts a bus command, leaving timetable reads to the serial queue.
pub(crate) fn run(name: &str, args: &[String], token: ReplyToken) -> CommandReply {
    match name {
        "now" if args.is_empty() => CommandReply::Done(Ok(now_reply(data::now()))),
        "now" => CommandReply::Done(Err(usage("now takes no arguments"))),
        "stops" => {
            let query = args.join(" ").trim().to_string();
            if query.is_empty() {
                return CommandReply::Done(Err(usage("stops needs a query")));
            }
            ttqueue::submit(move || {
                let result = data::all_stops(worker::data_dir())
                    .map_err(|e| error(e.to_string()))
                    .map(|stops| {
                        let hits = picker::matching_stops(&stops, &query);
                        let rows: Vec<_> = hits
                            .iter()
                            .take(20)
                            .map(|&i| json!({"code": stops[i].code, "name": stops[i].name}))
                            .collect();
                        json!({
                            "ok": true,
                            "query": query,
                            "count": hits.len(),
                            "stops": rows
                        })
                    });
                post_json(token, result);
            });
            CommandReply::Pending
        }
        "lines" => {
            if args.len() != 1 {
                return CommandReply::Done(Err(usage("lines takes one stop code")));
            }
            let code = args[0].clone();
            ttqueue::submit(move || {
                let result = data::lines_at_stop(worker::data_dir(), &code)
                    .map_err(|e| error(e.to_string()))
                    .map(|lines| {
                        let rows: Vec<_> = lines
                            .iter()
                            .enumerate()
                            .map(|(i, line)| {
                                json!({
                                    "n": i + 1,
                                    "agency": line.agency,
                                    "line": line.line,
                                    "next": line.next,
                                    "headsign": line.headsign,
                                    "trips": line.trips
                                })
                            })
                            .collect();
                        json!({"ok": true, "stop_code": code, "lines": rows})
                    });
                post_json(token, result);
            });
            CommandReply::Pending
        }
        "choose" => {
            let parsed = match parse_choose(args) {
                Ok(parsed) => parsed,
                Err(message) => return CommandReply::Done(Err(usage(message))),
            };
            ttqueue::submit(move || {
                let result = resolve_choose(&parsed).map_err(|failure| match failure {
                    ChooseFailure::Usage(message) => usage(message),
                    ChooseFailure::Error(message) => error(message),
                });
                post_choose(token, result);
            });
            CommandReply::Pending
        }
        _ => CommandReply::Done(Err(usage("unknown command"))),
    }
}

/// Delivers a queued command result and applies a chosen config on the UI thread.
pub(crate) fn finish(posted: Posted) {
    match posted.result {
        PostedResult::Json(result) => posted.token.finish(result),
        PostedResult::Choose(Err(e)) => posted.token.finish(Err(e)),
        PostedResult::Choose(Ok(chosen)) => {
            let cfg = picker::chosen_config(&chosen.stop_name, &chosen.stop_code, &chosen.line);
            let changed = worker::current().0 != cfg;
            match worker::set_config(cfg.clone()) {
                Ok(()) => posted.token.finish(Ok(json!({
                    "ok": true,
                    "changed": changed,
                    "config": {
                        "agency": cfg.agency,
                        "line": cfg.line,
                        "stop_code": cfg.stop_code,
                        "label": cfg.label
                    }
                }))),
                Err(e) => posted
                    .token
                    .finish(Err(error(format!("saving settings failed: {e}")))),
            }
        }
    }
}

/// Posts a JSON command result for delivery on the UI thread.
fn post_json(token: ReplyToken, result: Result<Value, CommandError>) {
    post(Posted {
        token,
        result: PostedResult::Json(result),
    });
}

/// Posts a resolved `choose` result for delivery on the UI thread.
fn post_choose(token: ReplyToken, result: Result<Chosen, CommandError>) {
    post(Posted {
        token,
        result: PostedResult::Choose(result),
    });
}

/// Posts a completed command and frees its payload if Windows rejects the message.
fn post(posted: Posted) {
    let main = worker::MAIN.load(std::sync::atomic::Ordering::Relaxed);
    let raw = Box::into_raw(Box::new(posted));
    if unsafe { PostMessageW(main, WM_COMMAND_DONE, 0, raw as isize) } == 0 {
        unsafe {
            drop(Box::from_raw(raw));
        }
    }
}

/// Creates an error marked for the command layer to append its usage line.
fn usage(message: impl Into<String>) -> CommandError {
    CommandError {
        usage: true,
        message: message.into(),
    }
}

/// Creates an operational command error without usage text.
fn error(message: String) -> CommandError {
    CommandError {
        usage: false,
        message,
    }
}

/// Parses the stop code, line, optional agency and optional direction selector.
fn parse_choose(args: &[String]) -> Result<ChooseArgs, String> {
    if args.len() < 2 {
        return Err("choose needs a stop code and a line".into());
    }
    let (code, line) = (args[0].clone(), args[1].clone());
    let mut agency = None;
    let mut to = None;
    let mut i = 2;
    if i < args.len() && args[i] != "--to" {
        agency = Some(args[i].clone());
        i += 1;
    }
    if i < args.len() {
        if args[i] != "--to" {
            return Err(format!("unexpected argument {}", args[i]));
        }
        if i + 1 >= args.len() {
            return Err("--to needs a value".into());
        }
        to = Some(args[i + 1].clone());
        if i + 2 < args.len() {
            return Err(format!("unexpected argument {}", args[i + 2]));
        }
    }
    Ok(ChooseArgs {
        code,
        line,
        agency,
        to,
    })
}

/// Looks up a stop and resolves a requested line and direction from the local timetable.
fn resolve_choose(args: &ChooseArgs) -> Result<Chosen, ChooseFailure> {
    let stops = data::all_stops(worker::data_dir())
        .map_err(|e| ChooseFailure::Error(e.to_string()))?;
    let stop = stops
        .iter()
        .find(|stop| stop.code == args.code)
        .ok_or_else(|| ChooseFailure::Error(format!("no stop with code {}", args.code)))?;
    let lines = data::lines_at_stop(worker::data_dir(), &args.code)
        .map_err(|e| ChooseFailure::Error(e.to_string()))?;
    let here = lines_at_description(&lines);
    let candidates = filter_candidates(&lines, args);
    if candidates.is_empty() {
        return Err(ChooseFailure::Error(format!(
            "line {} doesn't stop at {}; lines here: {}",
            args.line, args.code, here
        )));
    }
    let selected = select_candidate(&candidates, args).map_err(ChooseFailure::Usage)?;
    let line = &candidates[selected];
    Ok(Chosen {
        stop_name: stop.name.clone(),
        stop_code: stop.code.clone(),
        line: line.clone(),
    })
}

/// Formats distinct agency and line pairs in their timetable result order.
fn lines_at_description(lines: &[data::LineOpt]) -> String {
    let mut seen = HashSet::new();
    let pairs: Vec<String> = lines
        .iter()
        .filter_map(|line| {
            let key = (line.agency.as_str(), line.line.as_str());
            seen.insert(key)
                .then(|| format!("{} {}", line.agency, line.line))
        })
        .collect();
    if pairs.is_empty() {
        "(none)".into()
    } else {
        pairs.join(", ")
    }
}

/// Filters timetable directions by the requested line and optional agency.
fn filter_candidates(lines: &[data::LineOpt], args: &ChooseArgs) -> Vec<data::LineOpt> {
    lines
        .iter()
        .filter(|item| {
            item.line.eq_ignore_ascii_case(&args.line)
                && args.agency.as_ref().map_or(true, |agency| {
                    item.agency.eq_ignore_ascii_case(agency)
                })
        })
        .cloned()
        .collect()
}

/// Chooses a direction when its index or destination identifies exactly one candidate.
fn select_candidate(candidates: &[data::LineOpt], args: &ChooseArgs) -> Result<usize, String> {
    let mut matches: Vec<usize> = (0..candidates.len()).collect();
    if let Some(to) = args.to.as_deref() {
        if let Ok(n) = to.parse::<usize>() {
            if n == 0 || n > candidates.len() {
                return Err(candidate_list(&args.line, &args.code, candidates));
            }
            matches = vec![n - 1];
        } else {
            matches = candidates
                .iter()
                .enumerate()
                .filter(|(_, line)| {
                    line.headsign.eq_ignore_ascii_case(to) || line.next.eq_ignore_ascii_case(to)
                })
                .map(|(i, _)| i)
                .collect();
            if matches.is_empty() {
                matches = candidates
                    .iter()
                    .enumerate()
                    .filter(|(_, line)| {
                        line.headsign.to_lowercase().contains(&to.to_lowercase())
                            || line.next.to_lowercase().contains(&to.to_lowercase())
                    })
                    .map(|(i, _)| i)
                    .collect();
            }
            if matches.is_empty() {
                return Err(candidate_list(&args.line, &args.code, candidates));
            }
        }
    }
    if matches.len() == 1 {
        return Ok(matches[0]);
    }
    Err(candidate_list(&args.line, &args.code, candidates))
}

/// Formats the unchanged ambiguity and candidate-selection message.
fn candidate_list(line: &str, code: &str, candidates: &[data::LineOpt]) -> String {
    let multiple_agencies = candidates
        .iter()
        .map(|candidate| candidate.agency.as_str())
        .collect::<HashSet<_>>()
        .len()
        > 1;
    let mut message = format!(
        "line {line} has {} directions at {code}; pick one with --to <n|text>:",
        candidates.len()
    );
    for (i, candidate) in candidates.iter().enumerate() {
        let next = if candidate.next.is_empty() {
            "trip ends here"
        } else {
            &candidate.next
        };
        if multiple_agencies {
            message.push_str(&format!(
                "\n{}. {} · {} · {}",
                i + 1,
                candidate.agency,
                next,
                candidate.headsign
            ));
        } else {
            message.push_str(&format!(
                "\n{}. {} · {}",
                i + 1,
                next,
                candidate.headsign
            ));
        }
    }
    message
}

/// Builds the current departures reply from worker-filtered state and the no-live flag.
fn now_reply(now: i64) -> Value {
    let (cfg, sched, live, ok_at, err) = worker::current();
    let no_live = worker::shared().lock().unwrap().no_live;
    now_reply_from(&cfg, sched.as_deref(), &live, ok_at, err.as_deref(), no_live, now)
}

/// Builds a departures reply from supplied state without performing I/O or freshness filtering.
fn now_reply_from(
    cfg: &data::Config,
    sched: Option<&data::Schedule>,
    live: &[data::Bus],
    ok_at: Option<i64>,
    err: Option<&str>,
    no_live: bool,
    now: i64,
) -> Value {
    let departures: Vec<_> = data::upcoming(sched, live, now)
        .into_iter()
        .take(6)
        .map(|bus| {
            let time = data::clock(bus.t, "%H:%M");
            let planned = data::clock(bus.plan, "%H:%M");
            json!({
                "time": time,
                "in_min": ((bus.t - now) as f64 / 60.0).round() as i64,
                "planned": planned,
                "delay_min": ((bus.t - bus.plan) as f64 / 60.0).round() as i64,
                "live": bus.live,
                "cancelled": bus.cancel,
                "headsign": if cfg.dest.is_empty() { bus.head } else { cfg.dest.clone() }
            })
        })
        .collect();
    json!({
        "ok": true,
        "agency": cfg.agency,
        "line": cfg.line,
        "stop_code": cfg.stop_code,
        "label": cfg.label,
        "dest": cfg.dest,
        "live_age_s": ok_at.map(|at| now - at),
        "live_error": err,
        "no_live": no_live,
        "departures": departures
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a line option for command unit tests.
    fn line(agency: &str, line: &str, next: &str, headsign: &str) -> data::LineOpt {
        data::LineOpt {
            agency: agency.into(),
            line: line.into(),
            next: next.into(),
            headsign: headsign.into(),
            trips: 1,
            color: String::new(),
            text_color: String::new(),
        }
    }

    /// Builds the common choose arguments used by unit tests.
    fn args(to: Option<&str>) -> ChooseArgs {
        ChooseArgs {
            code: "123".into(),
            line: "3".into(),
            agency: None,
            to: to.map(str::to_string),
        }
    }

    /// Verifies valid and invalid choose command forms.
    #[test]
    fn parses_choose_arguments() {
        assert_eq!(parse_choose(&["123".into(), "3".into()]).unwrap(), args(None));
        assert_eq!(
            parse_choose(&[
                "123".into(),
                "3".into(),
                "UOV".into(),
                "--to".into(),
                "Utrecht".into()
            ])
            .unwrap(),
            ChooseArgs {
                agency: Some("UOV".into()),
                to: Some("Utrecht".into()),
                ..args(None)
            }
        );
        assert_eq!(
            parse_choose(&["123".into()]).unwrap_err(),
            "choose needs a stop code and a line"
        );
        assert_eq!(
            parse_choose(&["123".into(), "3".into(), "--to".into()]).unwrap_err(),
            "--to needs a value"
        );
        assert_eq!(
            parse_choose(&["123".into(), "3".into(), "UOV".into(), "extra".into()])
                .unwrap_err(),
            "unexpected argument extra"
        );
    }

    /// Verifies case-insensitive line and optional agency filtering.
    #[test]
    fn filters_choose_candidates_by_line_and_agency() {
        let lines = vec![
            line("UOV", "3", "Central", "North"),
            line("QBUZZ", "3", "South", "South"),
            line("UOV", "4", "East", "East"),
        ];
        let mut query = args(None);
        assert_eq!(filter_candidates(&lines, &query).len(), 2);
        query.agency = Some("uov".into());
        assert_eq!(filter_candidates(&lines, &query).len(), 1);
    }

    /// Verifies `--to` number, text, substring, ambiguity and no-match outcomes.
    #[test]
    fn chooses_direction_only_when_unambiguous() {
        let lines = vec![
            line("UOV", "3", "Central Station", "Northbound"),
            line("UOV", "3", "South", "Southbound"),
        ];
        assert!(select_candidate(&lines, &args(None))
            .unwrap_err()
            .contains("has 2 directions"));
        assert_eq!(select_candidate(&lines, &args(Some("2"))).unwrap(), 1);
        assert_eq!(select_candidate(&lines, &args(Some("central station"))).unwrap(), 0);
        assert_eq!(select_candidate(&lines, &args(Some("Central"))).unwrap(), 0);
        assert!(select_candidate(&lines, &args(Some("nope")))
            .unwrap_err()
            .contains("has 2 directions"));
    }

    /// Verifies the stop summary lists distinct agency and line pairs in order.
    #[test]
    fn describes_distinct_lines_at_stop_in_order() {
        let lines = vec![
            line("UOV", "3", "Central", "North"),
            line("UOV", "3", "South", "South"),
            line("QBUZZ", "8", "East", "East"),
            line("UOV", "4", "West", "West"),
            line("QBUZZ", "8", "North", "North"),
        ];
        assert_eq!(lines_at_description(&lines), "UOV 3, QBUZZ 8, UOV 4");
        assert_eq!(lines_at_description(&[]), "(none)");
    }

    /// Verifies current config and departures serialize from supplied live data without I/O.
    #[test]
    fn builds_now_reply_from_fake_buses() {
        let now = 1_791_234_000;
        let cfg = data::Config::default();
        let buses = vec![data::Bus {
            t: now + 149,
            plan: now + 120,
            live: true,
            cancel: false,
            passed: false,
            head: "Utrecht CS".into(),
        }];
        let reply = now_reply_from(&cfg, None, &buses, Some(now - 30), Some("offline"), false, now);
        assert_eq!(reply["ok"], true);
        assert_eq!(reply["live_age_s"], 30);
        assert_eq!(reply["live_error"], "offline");
        assert_eq!(reply["departures"][0]["in_min"], 2);
        assert_eq!(reply["departures"][0]["delay_min"], 0);
        assert_eq!(reply["departures"][0]["headsign"], "Utrecht CS");
    }
}
