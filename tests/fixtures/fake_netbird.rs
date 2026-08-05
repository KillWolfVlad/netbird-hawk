use std::{env, fs, path::PathBuf, process, thread, time::Duration};

fn main() {
    let state = PathBuf::from(env::var_os("FAKE_NETBIRD_STATE").expect("missing fake state path"));
    let behavior = state.with_extension("behavior");
    let behavior = fs::read_to_string(behavior).unwrap_or_else(|_| "ok".to_owned());
    let arguments = env::args().skip(1).collect::<Vec<_>>();

    if arguments.first().is_some_and(|argument| argument == "status") {
        match behavior.trim() {
            "status-failure" => process::exit(9),
            "status-timeout" => thread::sleep(Duration::from_secs(30)),
            "malformed-status" => {
                println!("Authorization: Bearer not-for-persistence");
                return;
            }
            _ => {}
        }
        let active = fs::read_to_string(&state).unwrap_or_else(|_| "alpha".to_owned());
        if arguments == ["status", "--json"] {
            println!(r#"{{"profileName":"{}","peers":[]}}"#, active.trim());
        } else if arguments == ["status"] {
            println!("Management: Connected");
            println!("Profile: {}", active.trim());
        } else {
            process::exit(64);
        }
        return;
    }

    if arguments.len() == 3 && arguments[0] == "profile" && arguments[1] == "select" {
        match behavior.trim() {
            "select-failure" => process::exit(8),
            "select-timeout" => thread::sleep(Duration::from_secs(30)),
            _ => {}
        }
        fs::write(state, &arguments[2]).expect("write selected fake profile");
        return;
    }

    eprintln!("unexpected fake NetBird arguments: {arguments:?}");
    process::exit(64);
}
