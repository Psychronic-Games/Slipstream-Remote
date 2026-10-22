//! Isolated meeting endpoints using an existing station profile read-only.
//! SLIPSTREAM_RELAY_ONLY=1 scripts/check.sh cargo run -p slipstream-session
//!   --example meeting_smoke -- /path/to/profile
use anyhow::{Context, Result, ensure};
use slipstream_core::{
    config::{ServerSettings, Store},
    meeting::Source,
};
use slipstream_session::meetings::{Command, Meeting, Video};
use std::time::Duration;

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> Result<()> {
    let path = std::env::args()
        .nth(1)
        .context("Pass the profile directory (settings are read only)")?;
    let mut settings = Store::open(path)?.load_settings();
    settings.server.use_public_relays = false;
    let mut host = Meeting::host(&settings.server, "Meeting check", "Relay check", 20).await?;
    if let Some(key) = &settings.server.key {
        ensure!(
            !serde_json::to_string(&host.invitation)?.contains(key),
            "Invitation contains a station credential"
        );
        for route in host.invitation.address.relay_urls() {
            if let Some(token) = route
                .query_pairs()
                .find(|(k, _)| k == "token")
                .map(|(_, v)| v.into_owned())
            {
                ensure!(
                    slipstream_core::meeting::valid_relay_token(key, &token),
                    "Invitation does not use a guest relay lease"
                );
            }
        }
    }
    let guest_settings = ServerSettings {
        use_public_relays: false,
        ..Default::default()
    };
    let mut guest = Meeting::join(
        host.invitation.clone(),
        &guest_settings,
        "Independent guest",
    )
    .await?;
    tokio::time::timeout(Duration::from_secs(20), async {
        while host.state.borrow().lobby.is_empty() {
            host.state.changed().await?;
        }
        anyhow::Ok(())
    })
    .await??;
    let id = host.state.borrow().lobby[0].id;
    host.command(Command::Admit(id));
    tokio::time::timeout(Duration::from_secs(20), async {
        while guest.state.borrow().waiting {
            guest.state.changed().await?;
        }
        anyhow::Ok(())
    })
    .await??;
    guest.command(Command::Chat("Meeting relay smoke".into()));
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(slipstream_session::meetings::Event::Chat(chat)) = host.events.recv().await
            {
                ensure!(chat.text == "Meeting relay smoke", "Meeting chat differed");
                break;
            }
        }
        anyhow::Ok(())
    })
    .await??;
    host.command(Command::Media {
        microphone: true,
        source: Source::Screen,
    });
    guest.command(Command::Subscribe(vec![1]));
    tokio::time::sleep(Duration::from_millis(500)).await;
    let mut audio = slipstream_audio::opus::Encoder::voice()?;
    let pcm = [1000i16; slipstream_audio::opus::FRAME_SAMPLES];
    let encoded = audio.encode(&pcm)?.to_vec();
    let mut packet = vec![0, 0];
    packet.extend_from_slice(&encoded);
    host.publisher.audio(packet.into());
    let received = tokio::time::timeout(Duration::from_secs(10), guest.audio.recv())
        .await?
        .context("No meeting audio")?;
    ensure!(
        received.participant == 1 && received.data.as_ref() == encoded,
        "Meeting audio differed"
    );
    let mut decoder = slipstream_audio::opus::Decoder::new()?;
    let mut decoded = [0; slipstream_audio::opus::FRAME_SAMPLES];
    decoder.decode(Some(&received.data), &mut decoded)?;
    let mut encoder = slipstream_codec::Encoder::new(Default::default());
    let raw = slipstream_codec::RawFrame {
        width: 64,
        height: 64,
        stride: 256,
        format: slipstream_codec::PixelFormat::Rgba,
        data: vec![120; 64 * 64 * 4],
    };
    let encoded = encoder.encode(&raw, true)?.context("No encoded frame")?;
    ensure!(
        host.publisher.video(Video {
            participant: 0,
            generation: 1,
            seq: 1,
            keyframe: true,
            width: 64,
            height: 64,
            data: encoded.data
        }),
        "Video queue was full"
    );
    let received = tokio::time::timeout(Duration::from_secs(10), guest.video.recv())
        .await?
        .context("No meeting video")?;
    let mut decoder = slipstream_codec::Decoder::new()?;
    let frame = decoder
        .decode(&received.data)?
        .context("Video did not decode")?;
    ensure!(
        (frame.width, frame.height) == (64, 64),
        "Meeting video dimensions differed"
    );
    host.command(Command::Leave);
    tokio::time::timeout(Duration::from_secs(10), async {
        while guest.state.borrow().ended.is_none() {
            guest.state.changed().await?;
        }
        anyhow::Ok(())
    })
    .await??;
    println!(
        "Meeting check passed: guest relay lease, admission, chat, Opus audio, H.264 video and teardown"
    );
    Ok(())
}
