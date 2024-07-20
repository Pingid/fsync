use clap::{Arg, ArgAction, Command};
use fsync::Synchronize;

fn main() {
    let matches = Command::new("fsync")
        .arg_required_else_help(true)
        .about("Synchronizes files between two directories")
        .arg(
            Arg::new("source")
                .required(true)
                .index(1)
                .help("Source directory"),
        )
        .arg(
            Arg::new("destination")
                .required(true)
                .index(2)
                .help("Destination directory"),
        )
        .arg(
            Arg::new("delete")
                .long("delete")
                .short('d')
                .action(ArgAction::SetTrue)
                .help("Delete files in the destination that are not in the source"),
        )
        .arg(
            Arg::new("check-content")
                .long("checkout-content")
                .short('c')
                .action(ArgAction::SetTrue)
                .help("Use checksums to compare files instead of modified time"),
        )
        .arg(
            Arg::new("skip-permissions")
                .long("skip-permissions")
                .action(ArgAction::SetTrue)
                .help("Skip copying file permissions"),
        )
        .arg(
            Arg::new("threads")
                .long("threads")
                .help("Number of threads to use defaults to rayon default threadpool"),
        )
        .get_matches();

    let source = matches.get_one::<String>("source").unwrap();
    let destination = matches.get_one::<String>("destination").unwrap();
    let delete = matches.get_flag("delete");
    let check_content = matches.get_flag("check-content");
    let skip_permissions = matches.get_flag("skip-permissions");
    let threads = matches
        .get_one::<String>("threads")
        .and_then(|x| x.parse::<u8>().ok());

    let sync = Synchronize::new(source, destination)
        .delete(delete)
        .num_threads(threads)
        .check_content(check_content)
        .display_progress(true)
        .skip_permissions(skip_permissions);

    match sync.sync() {
        Ok(_) => {}
        Err(e) => eprintln!("{:?}", e),
    }
}
