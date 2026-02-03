use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use pipewire as pw;
use pw::spa::param::audio::{AudioFormat, AudioInfoRaw};
use pw::spa::pod::Pod;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Initializing PipeWire...");
    pw::init();
    
    let main_loop = pw::main_loop::MainLoop::new(None)?;
    let context = pw::context::Context::new(&main_loop)?;
    let core = context.connect(None)?;
    
    println!("Creating stream...");
    
    let buffer: Arc<Mutex<usize>> = Arc::new(Mutex::new(0));
    let buffer_clone = buffer.clone();
    
    let stream = pw::stream::Stream::new(
        &core,
        "test-capture",
        pw::properties::properties! {
            *pw::keys::MEDIA_TYPE => "Audio",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Music",
        },
    )?;
    
    println!("Setting up listener...");
    
    let _listener = stream
        .add_local_listener_with_user_data(())
        .state_changed(|_, _, old_state, new_state| {
            println!("State changed: {:?} -> {:?}", old_state, new_state);
        })
        .param_changed(|_, _, id, param| {
            println!("Param changed: id={:?}, param={:?}", id, param.is_some());
        })
        .process(move |stream, _user_data| {
            println!("Process callback triggered!");
            if let Some(mut buffer_data) = stream.dequeue_buffer() {
                let datas = buffer_data.datas_mut();
                if let Some(data) = datas.first_mut() {
                    let chunk = data.chunk();
                    let size = chunk.size() as usize;
                    println!("  Received {} bytes", size);
                    
                    if let Some(_samples) = data.data() {
                        let mut count = buffer_clone.lock().unwrap();
                        *count += size;
                    }
                }
            }
        })
        .register()?;
    
    println!("Configuring audio format...");
    
    let mut audio_info = AudioInfoRaw::new();
    audio_info.set_format(AudioFormat::S32LE);
    audio_info.set_rate(96000);
    audio_info.set_channels(2);
    
    let obj = pw::spa::pod::Object {
        type_: pw::spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
        id: pw::spa::param::ParamType::EnumFormat.as_raw(),
        properties: audio_info.into(),
    };
    
    let values: Vec<u8> = pw::spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &pw::spa::pod::Value::Object(obj),
    )?
    .0
    .into_inner();
    
    let mut params = [Pod::from_bytes(&values).unwrap()];
    
    println!("Connecting stream...");
    
    stream.connect(
        pw::spa::utils::Direction::Input,
        None,
        pw::stream::StreamFlags::AUTOCONNECT
            | pw::stream::StreamFlags::MAP_BUFFERS
            | pw::stream::StreamFlags::RT_PROCESS,
        &mut params,
    )?;
    
    println!("Stream connected! Running main loop...");
    println!("Target: riaa (auto-connect)");
    println!("Press Ctrl+C to stop");
    println!();
    
    main_loop.run();
    
    let total_bytes = *buffer.lock().unwrap();
    println!();
    println!("Test stopped!");
    println!("Total bytes received: {}", total_bytes);
    
    Ok(())
}
