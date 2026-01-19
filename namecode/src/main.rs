//! Command-line interface for namecode encoding/decoding.

use std::io::{self, BufRead, Write};

fn print_usage() {
    eprintln!("namecode - Encode Unicode strings as valid programming identifiers");
    eprintln!();
    eprintln!("Usage:");
    eprintln!("  namecode encode <string>     Encode a string");
    eprintln!("  namecode decode <string>     Decode a namecode string");
    eprintln!("  namecode encode              Read strings from stdin, one per line");
    eprintln!("  namecode decode              Read encoded strings from stdin");
    eprintln!();
    eprintln!("Examples:");
    eprintln!("  namecode encode 'hello world'");
    eprintln!("  namecode decode '_N_helloworld__fa0b'");
    eprintln!("  echo 'foo-bar' | namecode encode");
}

fn main() {
    let args: Vec<String> = std::env::args().collect();

    if args.len() < 2 {
        print_usage();
        std::process::exit(1);
    }

    match args[1].as_str() {
        "encode" => {
            if args.len() > 2 {
                // Encode arguments
                for arg in &args[2..] {
                    println!("{}", namecode::encode(arg));
                }
            } else {
                // Read from stdin
                let stdin = io::stdin();
                let stdout = io::stdout();
                let mut stdout = stdout.lock();

                for line in stdin.lock().lines() {
                    match line {
                        Ok(s) => {
                            let _ = writeln!(stdout, "{}", namecode::encode(&s));
                        }
                        Err(e) => {
                            eprintln!("Error reading input: {}", e);
                            std::process::exit(1);
                        }
                    }
                }
            }
        }
        "decode" => {
            if args.len() > 2 {
                // Decode arguments
                for arg in &args[2..] {
                    match namecode::decode(arg) {
                        Ok(decoded) => println!("{}", decoded),
                        Err(e) => {
                            eprintln!("Error decoding '{}': {}", arg, e);
                            std::process::exit(1);
                        }
                    }
                }
            } else {
                // Read from stdin
                let stdin = io::stdin();
                let stdout = io::stdout();
                let mut stdout = stdout.lock();

                for line in stdin.lock().lines() {
                    match line {
                        Ok(s) => match namecode::decode(&s) {
                            Ok(decoded) => {
                                let _ = writeln!(stdout, "{}", decoded);
                            }
                            Err(e) => {
                                eprintln!("Error decoding '{}': {}", s, e);
                                std::process::exit(1);
                            }
                        },
                        Err(e) => {
                            eprintln!("Error reading input: {}", e);
                            std::process::exit(1);
                        }
                    }
                }
            }
        }
        "help" | "--help" | "-h" => {
            print_usage();
        }
        _ => {
            eprintln!("Unknown command: {}", args[1]);
            print_usage();
            std::process::exit(1);
        }
    }
}
