use std::io;

fn main() {
    println!("{{\"event\":\"started\"}}");
    let mut line = String::new();
    if io::stdin().read_line(&mut line).is_err() {
        eprintln!("{{\"error\":\"stdin_read_failed\"}}");
        std::process::exit(1);
    }
    println!("{{\"event\":\"stopped\"}}");
}
