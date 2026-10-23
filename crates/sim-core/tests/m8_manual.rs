//! The manual token game.

use std::path::PathBuf;
use std::sync::Arc;

use sim_core::manual::{Action, Game, Pending, TokenState};
use sim_core::*;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(name)
}

/// A game that runs through tasks by itself, so the tests can focus on the decisions.
fn game(name: &str) -> Game {
    let mut g = Game::new(Arc::new(Project::open_main(&fixture(name)).unwrap()));
    g.stop_at_tasks = false;
    g
}

fn q(file: &str, id: &str) -> QId {
    QId::new(file, id)
}

fn start(g: &mut Game, file: &str, id: &str) {
    g.apply(Action::Start { event: q(file, id) }).unwrap();
}

/// The single pending choice (token, flows).
fn choice(g: &Game) -> (u64, Vec<String>) {
    let c: Vec<_> = g
        .pending()
        .into_iter()
        .filter_map(|p| match p {
            Pending::Choose { token, flows, .. } => {
                Some((token, flows.iter().map(|f| f.id.clone()).collect()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(c.len(), 1, "{:?}", g.pending());
    c.into_iter().next().unwrap()
}

fn choose(g: &mut Game, file: &str, flows: &[&str]) {
    let (token, _) = choice(g);
    g.apply(Action::Choose {
        token,
        flows: flows.iter().map(|f| q(file, f)).collect(),
    })
    .unwrap();
}

fn at(g: &Game) -> Vec<String> {
    let mut v: Vec<String> = g.tokens().iter().map(|t| t.node.id.clone()).collect();
    v.sort();
    v
}

#[test]
fn xor_choice_decides_the_path() {
    let f = "v13-model.bpmn";
    let mut g = game(f);
    assert_eq!(g.start_events(), [q(f, "Start")]);
    start(&mut g, f, "Start");
    assert_eq!(
        at(&g),
        ["Split"],
        "tasks pass by themselves; the token waits at the XOR split"
    );
    assert_eq!(choice(&g).1, ["Flow_simple", "Flow_complex"]);
    choose(&mut g, f, &["Flow_simple"]);
    assert!(g.tokens().is_empty(), "{:?}", g.tokens());
    assert_eq!(g.finished().len(), 1);
    assert!(
        g.log.iter().any(|l| l == "'Complex?': take simple"),
        "{:?}",
        g.log
    );
}

#[test]
fn and_split_and_join_run_by_themselves() {
    let f = "v13-model.bpmn";
    let mut g = game(f);
    start(&mut g, f, "Start");
    choose(&mut g, f, &["Flow_complex"]);
    assert!(g.tokens().is_empty());
    assert_eq!(g.finished().len(), 1);
}

#[test]
fn tasks_wait_for_the_player_by_default() {
    let g = Game::new(Arc::new(
        Project::open_main(&fixture("v13-model.bpmn")).unwrap(),
    ));
    assert!(g.stop_at_tasks);
}

#[test]
fn stopping_at_tasks_waits_for_the_player() {
    let f = "v13-model.bpmn";
    let mut g = game(f);
    g.stop_at_tasks = true;
    start(&mut g, f, "Start");
    assert_eq!(at(&g), ["Task_Intake"]);
    let Pending::Complete { token, .. } = g.pending()[0].clone() else {
        panic!()
    };
    g.apply(Action::Complete { token }).unwrap();
    assert_eq!(at(&g), ["Split"]);
    choose(&mut g, f, &["Flow_complex"]);
    assert_eq!(at(&g), ["Task_Check", "Task_Review"]);
    assert!(g.tokens().iter().all(|t| t.state == TokenState::AtTask));
}

#[test]
fn invalid_choices_are_rejected() {
    let f = "v13-model.bpmn";
    let mut g = game(f);
    start(&mut g, f, "Start");
    let (token, _) = choice(&g);
    assert!(
        g.apply(Action::Choose {
            token,
            flows: vec![q(f, "Flow_simple"), q(f, "Flow_complex")]
        })
        .is_err(),
        "XOR takes one flow"
    );
    assert!(
        g.apply(Action::Choose {
            token,
            flows: vec![q(f, "F1")]
        })
        .is_err()
    );
    assert!(g.apply(Action::Complete { token }).is_err());
    assert!(
        g.apply(Action::Start {
            event: q(f, "Task_Intake")
        })
        .is_err()
    );
    assert_eq!(at(&g), ["Split"], "nothing changed");
}

#[test]
fn inclusive_split_takes_the_chosen_flows_and_the_join_waits_for_them() {
    let f = "or-split-join.bpmn";
    let mut g = game(f);
    start(&mut g, f, "Start");
    let (_, flows) = choice(&g);
    assert_eq!(flows.len(), 3);
    choose(&mut g, f, &["Flow_a", "Flow_c"]);
    assert!(
        g.tokens().is_empty(),
        "both chosen tokens met at the join and ended: {:?}",
        g.tokens()
    );
    assert_eq!(g.log.iter().filter(|l| l.contains("completed")).count(), 1);
}

#[test]
fn inclusive_join_waits_while_a_token_can_still_come() {
    let f = "or-upstream.bpmn";
    let mut g = game(f);
    start(&mut g, f, "Start");
    // Task_A's token waits at the join, the other token waits at the XOR: it might still come.
    assert_eq!(at(&g), ["Or_Join", "Xor"]);
    choose(&mut g, f, &["Flow_toX"]);
    // The other token went to End_X; the join fired and the case finished.
    assert!(g.tokens().is_empty(), "{:?}", g.tokens());
}

#[test]
fn event_based_gateway_waits_for_the_first_event() {
    let f = "event-gateway.bpmn";
    let mut g = game(f);
    start(&mut g, f, "Start_A");
    assert_eq!(at(&g), ["Ebg"]);
    let Pending::Fire { events, .. } = g.pending()[0].clone() else {
        panic!()
    };
    // Timer and signal can be fired by hand; the message comes from pool B in the model.
    assert_eq!(events, [q(f, "Wait_Timer"), q(f, "Catch_Sig")]);
    let token = g.tokens()[0].id;
    assert!(
        g.apply(Action::Fire {
            token,
            event: q(f, "Catch_Msg")
        })
        .is_err()
    );
    // B sends its message: the message path wins.
    start(&mut g, f, "Start_B");
    assert!(g.tokens().is_empty(), "{:?}", g.tokens());
    assert!(g.log.iter().any(|l| l.contains("message reaches")));

    // Or the player lets the timer happen.
    let mut g = game(f);
    start(&mut g, f, "Start_A");
    let token = g.tokens()[0].id;
    g.apply(Action::Fire {
        token,
        event: q(f, "Wait_Timer"),
    })
    .unwrap();
    assert!(g.tokens().is_empty());
    // A signal from C would have woken the gateway too.
    let mut g = game(f);
    start(&mut g, f, "Start_A");
    start(&mut g, f, "Start_C");
    assert!(g.tokens().is_empty());
}

#[test]
fn messages_between_pools_and_buffering() {
    let f = "v7-message.bpmn";
    // Receiver first: it waits, the sender's message releases it.
    let mut g = game(f);
    start(&mut g, f, "Start_B");
    assert_eq!(at(&g), ["Catch"]);
    start(&mut g, f, "Start_A");
    assert!(g.tokens().is_empty());
    // Sender first: the message is kept and taken when the receiver gets there.
    let mut g = game(f);
    start(&mut g, f, "Start_A");
    start(&mut g, f, "Start_B");
    assert!(g.tokens().is_empty(), "{:?}", g.tokens());
}

#[test]
fn the_multi_file_case_can_be_played_to_the_end() {
    let f = "main.bpmn";
    let mut g = game("multi-file/main.bpmn");
    start(&mut g, f, "TimerStart");
    // The order goes to Ops, which calls subprocess A: its XOR asks.
    let (token, flows) = choice(&g);
    assert_eq!(flows, ["Flow_ok", "Flow_retry"]);
    let gw = g.tokens().into_iter().find(|t| t.id == token).unwrap();
    assert_eq!(gw.node, q("subprocess-a.bpmn", "Gateway_Check"));
    // Loop once via the link events, then go on.
    choose(&mut g, "subprocess-a.bpmn", &["Flow_retry"]);
    choose(&mut g, "subprocess-a.bpmn", &["Flow_ok"]);
    // Then billing calls subprocess B.
    let (token, _) = choice(&g);
    assert_eq!(
        g.tokens()
            .into_iter()
            .find(|t| t.id == token)
            .unwrap()
            .node
            .file,
        "subprocess-b.bpmn"
    );
    choose(&mut g, "subprocess-b.bpmn", &["Flow_ok"]);
    assert!(g.tokens().is_empty(), "{:?}", g.tokens());
    assert!(g.log.iter().any(|l| l.contains("message to black box")));
    // Only the main file's start event is offered: the subprocesses start through their calls.
    assert_eq!(g.start_events(), [q(f, "TimerStart")]);
    assert!(g.finished().len() >= 6, "{:?}", g.finished());
}

#[test]
fn boundary_events_can_be_fired_on_a_running_subprocess() {
    let f = "subprocess-boundary.bpmn";
    let mut g = game(f);
    g.stop_at_tasks = true;
    start(&mut g, f, "Start");
    assert_eq!(at(&g), ["Sub", "Task_In"]);
    let fire = g
        .pending()
        .into_iter()
        .find_map(|p| match p {
            Pending::Fire { token, events, .. } if events.contains(&q(f, "Timeout")) => Some(token),
            _ => None,
        })
        .expect("the timer boundary event can be fired");
    g.apply(Action::Fire {
        token: fire,
        event: q(f, "Timeout"),
    })
    .unwrap();
    assert!(
        g.tokens().is_empty(),
        "the subprocess and its task were cancelled: {:?}",
        g.tokens()
    );
    assert!(g.log.iter().any(|l| l.contains("'Timeout' happens")));
}

#[test]
fn a_clone_is_an_undo_point() {
    let f = "v13-model.bpmn";
    let mut g = game(f);
    start(&mut g, f, "Start");
    let before = g.clone();
    choose(&mut g, f, &["Flow_simple"]);
    assert!(g.tokens().is_empty());
    let g = before;
    assert_eq!(at(&g), ["Split"]);
}

#[test]
fn focus_follows_the_token_into_called_processes_and_back() {
    let mut g = Game::new(Arc::new(
        Project::open_main(&fixture("multi-file/main.bpmn")).unwrap(),
    ));
    start(&mut g, "main.bpmn", "TimerStart");
    assert_eq!(g.focus(), Some(q("main.bpmn", "Task_PlaceOrder")));
    // Play: always act on the token in focus (complete tasks, take "ok").
    let mut files = vec![g.focus().unwrap().file];
    let mut calls_seen: Vec<Vec<String>> = Vec::new();
    for _ in 0..60 {
        let Some(f) = g.focus() else { break };
        let action = g
            .pending()
            .into_iter()
            .find_map(|p| match p {
                Pending::Complete { token, task } if task == f => Some(Action::Complete { token }),
                Pending::Choose {
                    token,
                    gateway,
                    flows,
                    ..
                } if gateway == f => Some(Action::Choose {
                    token,
                    flows: vec![flows[0].clone()],
                }),
                Pending::Fire { token, at, events } if at == f => Some(Action::Fire {
                    token,
                    event: events[0].clone(),
                }),
                _ => None,
            })
            .expect("the focus is something to act on");
        g.apply(action).unwrap();
        if let Some(f) = g.focus()
            && files.last() != Some(&f.file)
        {
            calls_seen.push(g.focus_calls().iter().map(|c| c.id.clone()).collect());
            files.push(f.file);
        }
    }
    // Into subprocess A when Call A is reached; when A ends the case runs on by itself through
    // the main file into subprocess B (called by Call B); then back to the main file.
    assert_eq!(
        files,
        [
            "main.bpmn",
            "subprocess-a.bpmn",
            "subprocess-b.bpmn",
            "main.bpmn"
        ]
    );
    let expected: Vec<Vec<String>> = vec![vec!["Call_A".into()], vec!["Call_B".into()], vec![]];
    assert_eq!(calls_seen, expected);
}
