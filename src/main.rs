use clap::{Arg, ArgAction, Command};
use fsync::Syncronize;

fn main() {
    let matches = Command::new("fsync")
        .about("Synchronizes directories")
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
            Arg::new("threads")
                .long("threads")
                .help("Number of threads to use defaults to rayon default threadpool"),
        )
        .get_matches();

    let source = matches.get_one::<String>("source").unwrap();
    let destination = matches.get_one::<String>("destination").unwrap();
    let delete = matches.get_flag("delete");
    let threads = matches
        .get_one::<String>("threads")
        .and_then(|x| x.parse::<u8>().ok());

    let sync = Syncronize::new(source, destination)
        .delete(delete)
        .num_threads(threads)
        .display_progress(true);

    match sync.sync() {
        Ok(_) => {}
        Err(e) => eprintln!("{:?}", e),
    }
}
