use autorec::audio_stream::discovery;
use std::process;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let show_help = args.len() > 1 && (args[1] == "-h" || args[1] == "--help");
    
    if show_help {
        print_help();
        process::exit(0);
    }
    
    let filter_backend = if args.len() > 1 {
        Some(args[1].to_lowercase())
    } else {
        None
    };
    
    println!("Available audio sources:\n");
    
    let all_sources = discovery::discover_all_sources();
    
    if all_sources.is_empty() {
        println!("No audio sources found.");
        println!("\nMake sure:");
        println!("  - PipeWire is running for pipewire sources");
        println!("  - ALSA devices are available");
        println!("  - Audio files (.wav, .mp3, .flac) exist in current directory");
        process::exit(1);
    }
    
    // Group by backend
    let mut by_backend: std::collections::HashMap<String, Vec<&discovery::AudioSource>> = 
        std::collections::HashMap::new();
    
    for source in &all_sources {
        by_backend.entry(source.backend.clone())
            .or_insert_with(Vec::new)
            .push(source);
    }
    
    // Display sources grouped by backend
    for backend in ["pipewire", "pwpipe", "alsa", "file"] {
        if let Some(sources) = by_backend.get(backend) {
            if filter_backend.is_none() || filter_backend.as_ref() == Some(&backend.to_string()) {
                println!("{}:", backend.to_uppercase());
                for source in sources {
                    println!("  {}", source.url);
                    if let Some(desc) = &source.description {
                        println!("    └─ {}", desc);
                    }
                }
                println!();
            }
        }
    }
    
    if filter_backend.is_some() {
        let backend = filter_backend.unwrap();
        if !by_backend.contains_key(&backend) {
            println!("No sources found for backend: {}", backend);
            process::exit(1);
        }
    }
}

fn print_help() {
    println!("show_sources - List available audio input sources");
    println!();
    println!("USAGE:");
    println!("    show_sources [BACKEND]");
    println!();
    println!("BACKENDS:");
    println!("    pipewire    Native PipeWire audio sources");
    println!("    pwpipe      PipeWire sources (subprocess mode)");
    println!("    alsa        ALSA audio devices");
    println!("    file        Audio files in current directory");
    println!();
    println!("EXAMPLES:");
    println!("    show_sources              List all available sources");
    println!("    show_sources pipewire     List only PipeWire sources");
    println!("    show_sources file         List only audio files");
}
