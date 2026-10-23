use super::*;
#[test]
fn invitation_routes_respect_both_stations_public_relay_policy() {
    let public = iroh::RelayMode::Default.relay_map().urls::<Vec<_>>()[0].clone();
    let private: iroh::RelayUrl = "https://meeting.example/?token=guest-lease"
        .parse()
        .unwrap();
    let mut invitation = Invitation {
        version: VERSION,
        address: iroh::EndpointAddr::new(iroh::SecretKey::generate().public())
            .with_relay_url(public.clone())
            .with_relay_url(private.clone()),
        token: random_secret(),
        title: "Policy".into(),
        expires: now() + 3600,
        public_relays: true,
    };
    let server = ServerSettings {
        key: Some("must-not-be-forwarded".into()),
        ..local()
    };
    let (settings, address) = guest_routes(&invitation, &server).unwrap();
    assert_eq!(settings.relays, vec![private.clone()]);
    assert!(settings.key.is_none() && settings.url.is_none());
    assert!(address.relay_urls().all(|url| url == &private));
    let mut allowed = server.clone();
    allowed.use_public_relays = true;
    assert_eq!(
        guest_routes(&invitation, &allowed).unwrap().1.addrs.len(),
        2
    );
    invitation.public_relays = false;
    assert_eq!(
        guest_routes(&invitation, &allowed).unwrap().1.addrs.len(),
        1
    );
    invitation.address = iroh::EndpointAddr::new(invitation.address.id).with_relay_url(public);
    assert!(guest_routes(&invitation, &allowed).is_err());
    // An explicit public-station meeting is a chosen station route, not a
    // fallback from the guest's private endpoint.
    let own_public: iroh::RelayUrl = slipstream_core::config::PUBLIC_STATION_URL.parse().unwrap();
    invitation.address = iroh::EndpointAddr::new(invitation.address.id).with_relay_url(own_public.clone());
    let (settings, address) = guest_routes(&invitation, &server).unwrap();
    assert!(settings.key.is_none());
    assert!(address.relay_urls().all(|url| url == &own_public));
}
fn local() -> ServerSettings {
    ServerSettings {
        use_public_relays: false,
        relays: Vec::new(),
        url: None,
        ..Default::default()
    }
}
async fn state(meeting: &mut Meeting, predicate: impl Fn(&State) -> bool) -> State {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let state = meeting.state.borrow().clone();
            if predicate(&state) {
                return state;
            }
            meeting.state.changed().await.unwrap();
        }
    })
    .await
    .unwrap_or_else(|_| panic!("meeting state timed out: {:?}", *meeting.state.borrow()))
}
async fn chat(meeting: &mut Meeting) -> Chat {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(Event::Chat(chat)) = meeting.events.recv().await {
                return chat;
            }
        }
    })
    .await
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn twenty_participant_meeting_admission_chat_media_removal_and_capacity() {
    let settings = local();
    let mut host = Meeting::host(&settings, "Organizer", "Support", 20)
        .await
        .unwrap();
    let mut guests = Vec::new();
    for i in 0..19 {
        guests.push(
            Meeting::join(host.invitation.clone(), &settings, &format!("Guest {i}"))
                .await
                .unwrap(),
        );
    }
    state(&mut host, |s| s.lobby.len() == 8).await;
    // The bounded lobby refuses overflow; admit in batches so every client can join.
    for guest in &mut guests {
        let _ = tokio::time::timeout(Duration::from_secs(2), guest.state.changed()).await;
    }
    drop(guests);
    state(&mut host, |s| s.lobby.is_empty()).await;
    let mut guests = Vec::new();
    for i in 0..19 {
        let mut guest = Meeting::join(host.invitation.clone(), &settings, &format!("Guest {i}"))
            .await
            .unwrap();
        let waiting = state(&mut guest, |s| s.local > 0).await;
        assert!(waiting.waiting);
        assert!(waiting.participants.is_empty());
        guest.command(Command::Chat("must not arrive".into()));
        host.command(Command::Admit(waiting.local));
        state(&mut guest, |s| !s.waiting).await;
        guests.push(guest);
    }
    state(&mut host, |s| s.participants.len() == 20).await;
    let mut overflow = Meeting::join(host.invitation.clone(), &settings, "Overflow")
        .await
        .unwrap();
    let ended = state(&mut overflow, |s| s.ended.is_some()).await;
    let reason = ended.ended.unwrap();
    assert!(reason.contains("full"), "{reason}");
    guests[0].command(Command::Chat("Hello room".into()));
    assert_eq!(chat(&mut host).await.text, "Hello room");
    assert_eq!(chat(&mut guests[18]).await.text, "Hello room");
    let source = guests[0].state.borrow().local;
    guests[0].command(Command::Media {
        microphone: true,
        source: Source::Camera,
    });
    state(&mut host, |s| {
        s.participants
            .iter()
            .any(|p| p.id == source && p.microphone)
    })
    .await;
    guests[18].command(Command::Subscribe(vec![source]));
    host.command(Command::Subscribe(vec![source]));
    tokio::time::sleep(Duration::from_millis(100)).await;
    guests[0]
        .publisher
        .audio(Bytes::from_static(&[0, 0, 1, 2, 3]));
    let packet = tokio::time::timeout(Duration::from_secs(3), guests[18].audio.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(packet.participant, source);
    assert!(packet.data.as_ref() == [1, 2, 3]);
    let mut encoder = slipstream_codec::Encoder::new(Default::default());
    let raw = slipstream_codec::RawFrame {
        width: 64,
        height: 64,
        stride: 256,
        format: slipstream_codec::PixelFormat::Rgba,
        data: vec![128; 64 * 64 * 4],
    };
    let encoded = encoder.encode(&raw, true).unwrap().unwrap();
    assert!(guests[0].publisher.video(Video {
        participant: 999,
        generation: 1,
        seq: 1,
        keyframe: true,
        width: 64,
        height: 64,
        data: encoded.data
    }));
    let video = tokio::time::timeout(Duration::from_secs(3), guests[18].video.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(video.participant, source);
    let mut decoder = slipstream_codec::Decoder::new().unwrap();
    let frame = decoder.decode(&video.data).unwrap().unwrap();
    assert_eq!((frame.width, frame.height), (64, 64));
    // More than two frames must cross both upload and subscriber credit
    // windows, proving acknowledgements replenish the bounded queues.
    for seq in 2..=6 {
        assert!(guests[0].publisher.video(Video {
            participant: 999,
            generation: 1,
            seq,
            keyframe: true,
            width: 64,
            height: 64,
            data: video.data.clone(),
        }));
        let next = tokio::time::timeout(Duration::from_secs(3), guests[18].video.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!((next.participant, next.seq), (source, seq));
    }
    host.command(Command::Remove(source));
    state(&mut host, |s| s.participants.len() == 19).await;
    state(&mut guests[0], |s| s.ended.is_some()).await;
    guests[0].command(Command::Chat("must not arrive after removal".into()));
    guests[18].command(Command::Chat("Still connected".into()));
    assert_eq!(chat(&mut host).await.text, "Still connected");
    host.command(Command::Lock(true));
    state(&mut host, |s| s.locked).await;
    let mut locked = Meeting::join(host.invitation.clone(), &settings, "Locked")
        .await
        .unwrap();
    assert!(
        state(&mut locked, |s| s.ended.is_some())
            .await
            .ended
            .unwrap()
            .contains("locked")
    );
    host.command(Command::Leave);
    state(&mut guests[18], |s| s.ended.is_some()).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn call_tickets_are_single_use_and_lobby_receives_no_content() {
    let settings = local();
    let host = Meeting::host(&settings, "Organizer", "Calls", 4)
        .await
        .unwrap();
    let mut pending = Meeting::join(host.invitation.clone(), &settings, "Waiting")
        .await
        .unwrap();
    let waiting = state(&mut pending, |s| s.local > 0).await;
    host.command(Command::Chat("Private room content".into()));
    assert!(
        tokio::time::timeout(Duration::from_millis(100), pending.events.recv())
            .await
            .is_err()
    );
    let ticket = host.call_invitation().await.unwrap();
    let mut admitted = Meeting::join(ticket.clone(), &settings, "Called user")
        .await
        .unwrap();
    state(&mut admitted, |s| !s.waiting).await;
    let mut replay = Meeting::join(ticket, &settings, "Replay").await.unwrap();
    state(&mut replay, |s| s.ended.is_some()).await;
    host.command(Command::Reject(waiting.local));
    state(&mut pending, |s| s.ended.is_some()).await;
    host.command(Command::Leave);
    state(&mut admitted, |s| s.ended.is_some()).await;
}
