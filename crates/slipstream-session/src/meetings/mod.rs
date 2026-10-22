//! Participant-hosted meetings. One upload per source; admission precedes media.
pub mod calls;
pub mod media;
#[cfg(test)]
mod tests;
mod wire;

use anyhow::{Context, Result, bail, ensure};
use bytes::Bytes;
use iroh::{
    Endpoint,
    endpoint::{Connection, RecvStream, SendStream},
};
pub use slipstream_core::meeting::{Chat, Invitation, Participant, Source, State, Video};
use slipstream_core::{config::ServerSettings, directory::DirectoryClient, meeting::*};
use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    sync::Arc,
    time::Duration,
};
use tokio::sync::{mpsc, oneshot, watch};

pub enum Event {
    Chat(Chat),
    Keyframe,
    Notice(String),
}
pub struct AudioPacket {
    pub participant: u64,
    pub sequence: u16,
    pub data: Bytes,
}
pub enum Command {
    Chat(String),
    Admit(u64),
    Reject(u64),
    Remove(u64),
    Lock(bool),
    Media { microphone: bool, source: Source },
    Subscribe(Vec<u64>),
    Keyframe(u64),
    CallInvitation(oneshot::Sender<Invitation>),
    Leave,
}
#[derive(Clone)]
pub struct Publisher {
    video: mpsc::Sender<Arc<Video>>,
    audio: mpsc::Sender<Bytes>,
}
impl Publisher {
    pub fn video(&self, video: Video) -> bool {
        self.video.try_send(Arc::new(video)).is_ok()
    }
    pub fn audio(&self, packet: Bytes) {
        let _ = self.audio.try_send(packet);
    }
}
pub struct Meeting {
    pub invitation: Invitation,
    pub state: watch::Receiver<State>,
    pub events: mpsc::Receiver<Event>,
    pub video: mpsc::Receiver<Arc<Video>>,
    pub audio: mpsc::Receiver<AudioPacket>,
    pub publisher: Publisher,
    commands: mpsc::Sender<Command>,
}
impl Meeting {
    pub async fn host(
        server: &ServerSettings,
        name: &str,
        title: &str,
        capacity: usize,
    ) -> Result<Self> {
        let name = clean_name(name)?;
        let title = title.trim();
        ensure!(
            !title.is_empty()
                && title.chars().count() <= 80
                && !title.chars().any(char::is_control),
            "Enter a meeting title of 1–80 characters"
        );
        let mut meeting_settings = server.clone();
        // The meeting endpoint itself uses only a guest lease, so even
        // transport address exchange cannot expose a station's master key.
        if let Some(url) = &server.url {
            let mut dc = DirectoryClient::new(url, server.key.clone())?;
            let hello = dc.hello().await?;
            meeting_settings.relays = hello.relays;
            meeting_settings.station_allows_public_relays = hello.public_relays;
            let lease = tokio::time::timeout(Duration::from_secs(12), dc.meeting_relay())
                .await
                .context("Station did not answer the meeting relay request")??;
            for relay in &mut meeting_settings.relays {
                let mut url: reqwest::Url = relay.clone().into();
                url.set_query(None);
                if let Some(token) = &lease {
                    url.query_pairs_mut().append_pair("token", token);
                }
                *relay = url.into();
            }
        } else {
            // Local/test meetings have no private station credentials.
            for relay in &meeting_settings.relays {
                ensure!(
                    relay.query().is_none(),
                    "A private meeting needs a station relay lease"
                );
            }
        }
        meeting_settings.key = None;
        meeting_settings.url = None;
        let endpoint = bind(&meeting_settings).await?;
        let invitation = Invitation {
            version: VERSION,
            address: endpoint.addr(),
            token: random_secret(),
            title: title.into(),
            expires: now() + 4 * 3600,
            public_relays: meeting_settings.public_relays_enabled(),
        };
        let state = State {
            title: title.into(),
            local: 1,
            host: true,
            capacity: capacity.clamp(2, MAX_CAPACITY),
            participants: vec![Participant {
                id: 1,
                name,
                microphone: false,
                source: Source::None,
            }],
            ..Default::default()
        };
        let (meeting, channels) = channels(invitation.clone(), state);
        tokio::spawn(host(endpoint, invitation, channels));
        Ok(meeting)
    }
    pub async fn join(invitation: Invitation, server: &ServerSettings, name: &str) -> Result<Self> {
        invitation.validate()?;
        let name = clean_name(name)?;
        let (settings, address) = guest_routes(&invitation, server)?;
        let endpoint = bind(&settings).await?;
        let conn = tokio::time::timeout(Duration::from_secs(20), endpoint.connect(address, ALPN))
            .await
            .context("Meeting host did not answer")?
            .context("Cannot reach this meeting; the host may need an update")?;
        let (mut send, recv) = conn.open_bi().await?;
        wire::write(
            &mut send,
            &ClientMessage::Join {
                version: VERSION,
                token: invitation.token.clone(),
                name,
            },
            MAX_CONTROL,
        )
        .await?;
        let state = State {
            title: invitation.title.clone(),
            waiting: true,
            ..Default::default()
        };
        let (meeting, channels) = channels(invitation.clone(), state);
        tokio::spawn(guest(endpoint, conn, send, recv, channels));
        Ok(meeting)
    }
    pub fn command(&self, cmd: Command) {
        // Reject local commands at submission time as well as enforcing
        // admission at the host. A queued lobby message must not be sent
        // just because an admission update wins the guest's next select.
        if self.state.borrow().waiting && !matches!(cmd, Command::Leave) {
            return;
        }
        let _ = self.commands.try_send(cmd);
    }
    pub async fn call_invitation(&self) -> Result<Invitation> {
        let (tx, rx) = oneshot::channel();
        self.commands.send(Command::CallInvitation(tx)).await?;
        Ok(tokio::time::timeout(Duration::from_secs(3), rx).await??)
    }
    pub fn command_sender(&self) -> mpsc::Sender<Command> {
        self.commands.clone()
    }
}
impl Drop for Meeting {
    fn drop(&mut self) {
        let _ = self.commands.try_send(Command::Leave);
    }
}

fn guest_routes(
    invitation: &Invitation,
    server: &ServerSettings,
) -> Result<(ServerSettings, iroh::EndpointAddr)> {
    let mut settings = server.clone();
    let mut address = invitation.address.clone();
    settings.use_public_relays &= invitation.public_relays;
    if !settings.public_relays_enabled() {
        let public = iroh::RelayMode::Default.relay_map();
        address.addrs.retain(|route| match route {
            iroh::TransportAddr::Relay(url) => {
                let legacy = public.urls::<Vec<_>>().iter().any(|known| known.host_str() == url.host_str());
                let backup = slipstream_core::config::is_public_station(url.as_str())
                    && !server.url.as_deref().is_some_and(slipstream_core::config::is_public_station);
                !legacy && !backup
            },
            _ => true,
        });
        ensure!(
            !address.addrs.is_empty(),
            "This meeting requires a public relay that your station has disabled"
        );
    }
    // Only invitation guest leases belong on this endpoint. The guest's
    // station key and configured station routes must never be substituted.
    settings.relays = address.relay_urls().cloned().collect();
    settings.key = None;
    settings.url = None;
    Ok((settings, address))
}

async fn bind(server: &ServerSettings) -> Result<Endpoint> {
    let endpoint =
        slipstream_core::net::bind_client(iroh::SecretKey::generate(), server, false).await?;
    endpoint.set_alpns(vec![ALPN.to_vec()]);
    // A newly bound endpoint can have no home relay in addr() yet. Wait
    // using the configured map so invitations include the chosen route.
    if !slipstream_core::net::relay_map(server).is_empty() {
        let _ = tokio::time::timeout(Duration::from_secs(5), endpoint.online()).await;
    }
    Ok(endpoint)
}
struct Channels {
    state: watch::Sender<State>,
    events: mpsc::Sender<Event>,
    video: mpsc::Sender<Arc<Video>>,
    audio: mpsc::Sender<AudioPacket>,
    commands: mpsc::Receiver<Command>,
    video_in: mpsc::Receiver<Arc<Video>>,
    audio_in: mpsc::Receiver<Bytes>,
}
fn channels(invitation: Invitation, state: State) -> (Meeting, Channels) {
    let (st, sr) = watch::channel(state);
    let (et, er) = mpsc::channel(128);
    let (vt, vr) = mpsc::channel(4);
    let (at, ar) = mpsc::channel(128);
    let (ct, cr) = mpsc::channel(64);
    let (vi, vir) = mpsc::channel(2);
    let (ai, air) = mpsc::channel(8);
    (
        Meeting {
            invitation,
            state: sr,
            events: er,
            video: vr,
            audio: ar,
            publisher: Publisher {
                video: vi,
                audio: ai,
            },
            commands: ct,
        },
        Channels {
            state: st,
            events: et,
            video: vt,
            audio: at,
            commands: cr,
            video_in: vir,
            audio_in: air,
        },
    )
}
struct Tasks(Vec<tokio::task::JoinHandle<()>>);
impl Drop for Tasks {
    fn drop(&mut self) {
        for task in &self.0 {
            task.abort();
        }
    }
}
struct Peer {
    member: Participant,
    admitted: bool,
    conn: Connection,
    control: mpsc::Sender<ServerMessage>,
    video: mpsc::Sender<Arc<Video>>,
    subscriptions: Vec<u64>,
    last: HashMap<u64, (u64, u64)>,
    pending: HashMap<u64, VecDeque<(u64, u64)>>,
    needs_keyframe: HashSet<u64>,
    budget: u32,
    _tasks: Tasks,
}
enum Incoming {
    Join {
        conn: Connection,
        send: SendStream,
        recv: RecvStream,
        token: String,
        name: String,
    },
    Message(u64, ClientMessage),
    Video(u64, Arc<Video>),
    Audio(u64, Bytes),
    Gone(u64),
}
struct Room {
    invitation: Invitation,
    channels: Channels,
    peers: BTreeMap<u64, Peer>,
    next: u64,
    subscriptions: Vec<u64>,
    tickets: HashMap<String, u64>,
    blocked: HashSet<iroh::EndpointId>,
    keyframes: HashMap<u64, std::time::Instant>,
}
impl Room {
    fn snapshot(&self, local: u64) -> State {
        let mut state = self.channels.state.borrow().clone();
        state.local = local;
        state.host = local == 1;
        state.waiting = local != 1 && self.peers.get(&local).is_none_or(|p| !p.admitted);
        state.participants.truncate(1);
        state.participants.extend(
            self.peers
                .values()
                .filter(|p| p.admitted)
                .map(|p| p.member.clone()),
        );
        state.lobby = if state.host {
            self.peers
                .values()
                .filter(|p| !p.admitted)
                .map(|p| p.member.clone())
                .collect()
        } else {
            Vec::new()
        };
        if state.waiting {
            state.participants.clear();
        }
        state
    }
    fn update(&mut self) {
        loop {
            self.channels.state.send_replace(self.snapshot(1));
            let mut slow = Vec::new();
            for (&id, peer) in &self.peers {
                if peer
                    .control
                    .try_send(ServerMessage::State(self.snapshot(id)))
                    .is_err()
                {
                    slow.push(id);
                }
            }
            if slow.is_empty() {
                break;
            }
            // Publish again after removal. Otherwise a closed writer can
            // leave its final lobby/member row in the organizer's snapshot.
            for id in slow {
                self.remove(
                    id,
                    "Connection is too slow to keep up with meeting controls",
                );
            }
        }
    }
    fn remove(&mut self, id: u64, reason: &str) {
        if let Some(peer) = self.peers.remove(&id) {
            let _ = peer.control.try_send(ServerMessage::Ended(reason.into()));
            peer.conn.close(2u32.into(), reason.as_bytes());
        }
        self.subscriptions.retain(|source| *source != id);
        self.keyframes.remove(&id);
        for peer in self.peers.values_mut() {
            peer.subscriptions.retain(|source| *source != id);
            peer.last.remove(&id);
            peer.pending.remove(&id);
            peer.needs_keyframe.remove(&id);
        }
    }
    fn admitted(&self, id: u64) -> bool {
        id == 1 || self.peers.get(&id).is_some_and(|p| p.admitted)
    }
    fn keyframe(&mut self, id: u64) {
        if self
            .keyframes
            .get(&id)
            .is_some_and(|at| at.elapsed() < Duration::from_secs(1))
        {
            return;
        }
        self.keyframes.insert(id, std::time::Instant::now());
        if id == 1 {
            let _ = self.channels.events.try_send(Event::Keyframe);
        } else if let Some(peer) = self.peers.get(&id) {
            let _ = peer.control.try_send(ServerMessage::Keyframe);
        }
    }
    fn message(&mut self, id: u64, msg: ClientMessage) {
        if !self.admitted(id) {
            return;
        }
        match msg {
            ClientMessage::Chat(text) => {
                if text.trim().is_empty()
                    || text.len() > 4000
                    || text
                        .chars()
                        .any(|c| c.is_control() && c != '\n' && c != '\t')
                {
                    return;
                }
                let name = if id == 1 {
                    self.channels.state.borrow().participants[0].name.clone()
                } else {
                    self.peers[&id].member.name.clone()
                };
                let chat = Chat {
                    sender: id,
                    name,
                    text,
                };
                let _ = self.channels.events.try_send(Event::Chat(chat.clone()));
                for peer in self.peers.values().filter(|p| p.admitted) {
                    let _ = peer.control.try_send(ServerMessage::Chat(chat.clone()));
                }
            }
            ClientMessage::Media { microphone, source } => {
                if id == 1 {
                    self.channels.state.send_modify(|s| {
                        s.participants[0].microphone = microphone;
                        s.participants[0].source = source;
                    });
                } else if let Some(peer) = self.peers.get_mut(&id) {
                    peer.member.microphone = microphone;
                    peer.member.source = source;
                }
                if source != Source::None {
                    self.keyframe(id);
                }
                self.update();
            }
            ClientMessage::Subscribe(mut ids) => {
                ids.retain(|source| *source != id && self.admitted(*source));
                ids.sort_unstable();
                ids.dedup();
                ids.truncate(MAX_SUBSCRIPTIONS);
                if id == 1 {
                    self.subscriptions = ids.clone();
                } else if let Some(peer) = self.peers.get_mut(&id) {
                    for &source in &ids {
                        if !peer.subscriptions.contains(&source) {
                            peer.needs_keyframe.insert(source);
                        }
                    }
                    peer.subscriptions = ids.clone();
                    peer.last.retain(|source, _| ids.contains(source));
                    peer.pending.retain(|source, _| ids.contains(source));
                    peer.needs_keyframe.retain(|source| ids.contains(source));
                }
                for source in ids {
                    self.keyframe(source);
                }
            }
            ClientMessage::Keyframe(source) => {
                if self.admitted(source) {
                    self.keyframe(source);
                }
            }
            ClientMessage::Leave => {
                self.remove(id, "Left the meeting");
                self.update();
            }
            ClientMessage::FrameAck {
                participant,
                generation,
                seq,
            } => {
                if let Some(peer) = self.peers.get_mut(&id)
                    && let Some(pending) = peer.pending.get_mut(&participant)
                    && let Some(at) = pending.iter().position(|value| *value == (generation, seq))
                {
                    pending.drain(..=at);
                }
            }
            ClientMessage::Join { .. } => {}
        }
    }
    fn audio(&mut self, id: u64, packet: Bytes) {
        if !self.admitted(id) || packet.len() < 3 || packet.len() > 1400 {
            return;
        }
        let microphone = if id == 1 {
            self.channels.state.borrow().participants[0].microphone
        } else {
            self.peers[&id].member.microphone
        };
        if !microphone {
            return;
        }
        let sequence = u16::from_le_bytes([packet[0], packet[1]]);
        if id != 1 {
            let _ = self.channels.audio.try_send(AudioPacket {
                participant: id,
                sequence,
                data: packet.slice(2..),
            });
        }
        let mut data = Vec::with_capacity(packet.len() + 9);
        data.push(AUDIO_TAG);
        data.extend_from_slice(&id.to_le_bytes());
        data.extend_from_slice(&packet);
        let data = Bytes::from(data);
        for (&other, peer) in &self.peers {
            if peer.admitted && other != id {
                let _ = peer.conn.send_datagram(data.clone());
            }
        }
    }
    fn video(&mut self, id: u64, video: Arc<Video>) {
        if !self.admitted(id) || video.validate().is_err() {
            return;
        }
        let source = if id == 1 {
            self.channels.state.borrow().participants[0].source
        } else {
            self.peers[&id].member.source
        };
        if source == Source::None {
            return;
        }
        let video = if video.participant == id {
            video
        } else {
            Arc::new(Video {
                participant: id,
                generation: video.generation,
                seq: video.seq,
                keyframe: video.keyframe,
                width: video.width,
                height: video.height,
                data: video.data.clone(),
            })
        };
        if id != 1 && self.subscriptions.contains(&id) {
            let _ = self.channels.video.try_send(video.clone());
        }
        let mut request = false;
        for (&other, peer) in &mut self.peers {
            if !peer.admitted || other == id || !peer.subscriptions.contains(&id) {
                continue;
            }
            if peer.last.get(&id).is_none_or(|(generation, seq)| {
                *generation != video.generation || seq.checked_add(1) != Some(video.seq)
            }) {
                peer.needs_keyframe.insert(id);
            }
            if peer.needs_keyframe.contains(&id) && !video.keyframe {
                request = true;
                continue;
            }
            if peer
                .pending
                .get(&id)
                .is_some_and(|pending| pending.len() >= 2)
            {
                peer.needs_keyframe.insert(id);
                request = true;
                continue;
            }
            if peer.video.try_send(video.clone()).is_err() {
                peer.needs_keyframe.insert(id);
                request = true;
            } else {
                peer.needs_keyframe.remove(&id);
                peer.last.insert(id, (video.generation, video.seq));
                peer.pending
                    .entry(id)
                    .or_default()
                    .push_back((video.generation, video.seq));
            }
        }
        if request {
            self.keyframe(id);
        }
    }
}
async fn host(endpoint: Endpoint, invitation: Invitation, channels: Channels) {
    let (tx, mut rx) = mpsc::channel(128);
    let accept_endpoint = endpoint.clone();
    let accept_tx = tx.clone();
    let accepting = tokio::spawn(async move {
        let slots = Arc::new(tokio::sync::Semaphore::new(40));
        let mut handshakes = tokio::task::JoinSet::new();
        loop {
            tokio::select! {
                Some(_)=handshakes.join_next(),if !handshakes.is_empty()=>{},
                incoming=accept_endpoint.accept()=>{
                    let Some(incoming)=incoming else{break;};let Ok(permit)=slots.clone().try_acquire_owned()else{incoming.refuse();continue;};
                    let tx=accept_tx.clone();
                    handshakes.spawn(async move{
                        let _permit=permit;
                        let work=async{
                            let conn=incoming.await?;ensure!(conn.alpn()==ALPN,"Unsupported meeting protocol");
                            let (send,mut recv)=conn.accept_bi().await?;
                            let ClientMessage::Join{version,token,name}=wire::read(&mut recv,MAX_CONTROL).await? else{bail!("Expected meeting join");};
                            ensure!(version==VERSION,"Update Slipstream");let name=clean_name(&name)?;
                            tx.send(Incoming::Join{conn,send,recv,token,name}).await?;anyhow::Ok(())
                        };
                        let _=tokio::time::timeout(Duration::from_secs(8),work).await;
                    });
                }
            }
        }
    });
    let _accept_guard = Tasks(vec![accepting]);
    let mut room = Room {
        invitation,
        channels,
        peers: BTreeMap::new(),
        next: 2,
        subscriptions: Vec::new(),
        tickets: HashMap::new(),
        blocked: HashSet::new(),
        keyframes: HashMap::new(),
    };
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    let reason = loop {
        tokio::select! {
            command=room.channels.commands.recv()=>{
                match command {
                    None|Some(Command::Leave)=>break "The organizer ended the meeting",
                    Some(Command::Chat(text))=>room.message(1,ClientMessage::Chat(text)),
                    Some(Command::Media{microphone,source})=>room.message(1,ClientMessage::Media{microphone,source}),
                    Some(Command::Subscribe(ids))=>room.message(1,ClientMessage::Subscribe(ids)),
                    Some(Command::Keyframe(id))=>room.keyframe(id),
                    Some(Command::Lock(locked))=>{room.channels.state.send_modify(|s|s.locked=locked);room.update();}
                    Some(Command::Admit(id))=>{
                        if room.snapshot(1).participants.len()<room.channels.state.borrow().capacity && let Some(peer)=room.peers.get_mut(&id){peer.admitted=true;}
                        room.update();
                    }
                    Some(Command::Reject(id))=>{room.remove(id,"The organizer declined your request");room.update();}
                    Some(Command::Remove(id))=>{
                        if let Some(peer)=room.peers.get(&id){room.blocked.insert(peer.conn.remote_id());}
                        room.remove(id,"The organizer removed you from the meeting");room.update();
                    }
                    Some(Command::CallInvitation(reply))=>{
                        room.tickets.retain(|_,expiry|*expiry>now());
                        if room.tickets.len()<64{
                            let mut invitation=room.invitation.clone();invitation.token=random_secret();invitation.expires=now()+120;
                            room.tickets.insert(invitation.token.clone(),invitation.expires);let _=reply.send(invitation);
                        }
                    }
                }
            }
            Some(video)=room.channels.video_in.recv()=>room.video(1,video),
            Some(audio)=room.channels.audio_in.recv()=>room.audio(1,audio),
            Some(event)=rx.recv()=>match event {
                Incoming::Join{conn,mut send,recv,token,name}=>{
                    let ticket=room.tickets.remove(&token).is_some_and(|expiry|expiry>now());
                    let state=room.snapshot(1);
                    let denied=if room.invitation.expires<=now(){Some("This meeting has expired")}
                        else if !ticket && !equal(token.as_bytes(),room.invitation.token.as_bytes()){Some("This meeting invitation is invalid or already used")}
                        else if room.blocked.contains(&conn.remote_id()){Some("You were removed from this meeting")}
                        else if state.locked{Some("The organizer has locked the meeting")}
                        else if state.participants.len()>=state.capacity{Some("The meeting is full")}
                        else if room.peers.values().filter(|p|!p.admitted).count()>=8{Some("The meeting lobby is full")}
                        else if room.peers.values().any(|p|p.conn.remote_id()==conn.remote_id()){Some("This client is already in the meeting")}
                        else{None};
                    if let Some(reason)=denied{
                        let _=tokio::time::timeout(Duration::from_millis(300),wire::write(&mut send,&ServerMessage::Ended(reason.into()),MAX_CONTROL)).await;
                        let _=send.finish();let _=tokio::time::timeout(Duration::from_millis(300),send.stopped()).await;
                        conn.close(2u32.into(),reason.as_bytes());continue;
                    }
                    let id=room.next;room.next+=1;
                    let peer=peer(id,conn,send,recv,Participant{id,name,microphone:false,source:Source::None},ticket,tx.clone());
                    room.peers.insert(id,peer);room.update();
                }
                Incoming::Message(id,msg)=>{
                    if let Some(peer)=room.peers.get_mut(&id){peer.budget+=1;if peer.budget>120{room.remove(id,"Too many meeting requests");room.update();continue;}}
                    room.message(id,msg);
                }
                Incoming::Video(id,video)=>{
                    let(generation,seq)=(video.generation,video.seq);room.video(id,video);
                    if let Some(peer)=room.peers.get(&id) && peer.admitted{let _=peer.control.try_send(ServerMessage::VideoAck{generation,seq});}
                }
                Incoming::Audio(id,audio)=>room.audio(id,audio),
                Incoming::Gone(id)=>{room.remove(id,"Connection lost");room.update();}
            },
            _=tick.tick()=>{
                for peer in room.peers.values_mut(){peer.budget=0;}
                room.tickets.retain(|_,expiry|*expiry>now());
                if room.invitation.expires<=now(){break "The meeting reached its four-hour limit; start a new meeting to continue";}
            }
        }
    };
    room.channels
        .state
        .send_modify(|s| s.ended = Some(reason.into()));
    for peer in room.peers.values() {
        let _ = peer.control.try_send(ServerMessage::Ended(reason.into()));
    }
    tokio::time::sleep(Duration::from_millis(150)).await;
    endpoint.close().await;
}
fn peer(
    id: u64,
    conn: Connection,
    mut send: SendStream,
    mut recv: RecvStream,
    member: Participant,
    admitted: bool,
    events: mpsc::Sender<Incoming>,
) -> Peer {
    send.set_priority(20).ok();
    let (ct, mut cr) = mpsc::channel(32);
    let (vt, mut vr) = mpsc::channel::<Arc<Video>>(2);
    let mut tasks = Vec::new();
    let c = conn.clone();
    tasks.push(tokio::spawn(async move {
        while let Some(msg) = cr.recv().await {
            if wire::write(&mut send, &msg, MAX_CONTROL).await.is_err() {
                break;
            }
        }
        c.close(0u32.into(), b"meeting control ended");
    }));
    let ev = events.clone();
    tasks.push(tokio::spawn(async move {
        while let Ok(msg) = wire::read(&mut recv, MAX_CONTROL).await {
            if ev.send(Incoming::Message(id, msg)).await.is_err() {
                return;
            }
        }
        let _ = ev.send(Incoming::Gone(id)).await;
    }));
    let c = conn.clone();
    tasks.push(tokio::spawn(async move {
        let Ok(mut send) = c.open_uni().await else {
            return;
        };
        send.set_priority(5).ok();
        while let Some(video) = vr.recv().await {
            if wire::write(&mut send, &*video, MAX_VIDEO).await.is_err() {
                break;
            }
        }
    }));
    let c = conn.clone();
    let ev = events.clone();
    tasks.push(tokio::spawn(async move {
        let Ok(mut recv) = c.accept_uni().await else {
            return;
        };
        while let Ok(video) = wire::read::<Video>(&mut recv, MAX_VIDEO).await {
            if video.validate().is_err() {
                c.close(0u32.into(), b"invalid video");
                break;
            }
            if ev.send(Incoming::Video(id, Arc::new(video))).await.is_err() {
                return;
            }
        }
    }));
    let c = conn.clone();
    tasks.push(tokio::spawn(async move {
        while let Ok(data) = c.read_datagram().await {
            if data.len() <= 1400 {
                let _ = events.try_send(Incoming::Audio(id, data));
            }
        }
    }));
    Peer {
        member,
        admitted,
        conn,
        control: ct,
        video: vt,
        subscriptions: Vec::new(),
        last: HashMap::new(),
        pending: HashMap::new(),
        needs_keyframe: HashSet::new(),
        budget: 0,
        _tasks: Tasks(tasks),
    }
}
async fn guest(
    endpoint: Endpoint,
    conn: Connection,
    mut send: SendStream,
    mut recv: RecvStream,
    mut channels: Channels,
) {
    send.set_priority(20).ok();
    let (upload_ack, mut upload_credit) = watch::channel((0u64, 0u64));
    let (down_ack, mut down_credit) = mpsc::channel(32);
    let (ct, mut cr) = mpsc::channel::<ServerMessage>(32);
    let mut tasks = Vec::new();
    tasks.push(tokio::spawn(async move {
        while let Ok(msg) = wire::read(&mut recv, MAX_CONTROL).await {
            if ct.send(msg).await.is_err() {
                break;
            }
        }
    }));
    let c = conn.clone();
    let video_out = channels.video.clone();
    tasks.push(tokio::spawn(async move {
        let Ok(mut recv) = c.accept_uni().await else {
            return;
        };
        while let Ok(video) = wire::read::<Video>(&mut recv, MAX_VIDEO).await {
            if video.validate().is_err() {
                break;
            }
            let ack = ClientMessage::FrameAck {
                participant: video.participant,
                generation: video.generation,
                seq: video.seq,
            };
            let _ = video_out.try_send(Arc::new(video));
            if down_ack.send(ack).await.is_err() {
                break;
            }
        }
    }));
    let c = conn.clone();
    let audio_out = channels.audio.clone();
    tasks.push(tokio::spawn(async move {
        while let Ok(data) = c.read_datagram().await {
            if data.len() >= 12 && data.len() <= 1500 && data[0] == AUDIO_TAG {
                let participant = u64::from_le_bytes(data[1..9].try_into().unwrap());
                let sequence = u16::from_le_bytes([data[9], data[10]]);
                let _ = audio_out.try_send(AudioPacket {
                    participant,
                    sequence,
                    data: data.slice(11..),
                });
            }
        }
    }));
    let c = conn.clone();
    let mut video_rx = channels.video_in;
    tasks.push(tokio::spawn(async move {
        let Ok(mut send) = c.open_uni().await else {
            return;
        };
        send.set_priority(5).ok();
        let mut pending = VecDeque::new();
        loop {
            let ack = *upload_credit.borrow_and_update();
            if let Some(at) = pending.iter().position(|value| *value == ack) {
                pending.drain(..=at);
            }
            if pending.len() >= 2 {
                if upload_credit.changed().await.is_err() {
                    break;
                }
                continue;
            }
            let Some(video) = video_rx.recv().await else {
                break;
            };
            if wire::write(&mut send, &*video, MAX_VIDEO).await.is_err() {
                break;
            }
            pending.push_back((video.generation, video.seq));
        }
    }));
    let _tasks = Tasks(tasks);
    let reason = loop {
        tokio::select! {
            message=cr.recv()=>match message{
                Some(ServerMessage::State(state))=>{channels.state.send_replace(state);}
                Some(ServerMessage::Chat(chat))=>{let _=channels.events.try_send(Event::Chat(chat));}
                Some(ServerMessage::Keyframe)=>{let _=channels.events.try_send(Event::Keyframe);}
                Some(ServerMessage::VideoAck{generation,seq})=>{upload_ack.send_replace((generation,seq));}
                Some(ServerMessage::Ended(reason))=>break reason,
                None=>break closed_reason(&conn),
            },
            command=channels.commands.recv()=>{
                let message=match command{
                    None|Some(Command::Leave)=>{let _=wire::write(&mut send,&ClientMessage::Leave,MAX_CONTROL).await;break "You left the meeting".into();}
                    Some(Command::Chat(text))=>ClientMessage::Chat(text),
                    Some(Command::Media{microphone,source})=>ClientMessage::Media{microphone,source},
                    Some(Command::Subscribe(ids))=>ClientMessage::Subscribe(ids),
                    Some(Command::Keyframe(id))=>ClientMessage::Keyframe(id),
                    _=>continue,
                };
                if wire::write(&mut send,&message,MAX_CONTROL).await.is_err(){break "Connection to the meeting host was lost".into();}
            }
            Some(audio)=channels.audio_in.recv()=>{
                if !channels.state.borrow().waiting{let _=conn.send_datagram(audio);}
            }
            Some(ack)=down_credit.recv()=>{if wire::write(&mut send,&ack,MAX_CONTROL).await.is_err(){break closed_reason(&conn);}}
            _=conn.closed()=>break closed_reason(&conn),
        }
    };
    channels.state.send_modify(|s| s.ended = Some(reason));
    endpoint.close().await;
}
fn closed_reason(conn: &Connection) -> String {
    if let Some(iroh::endpoint::ConnectionError::ApplicationClosed(reason)) = conn.close_reason()
        && !reason.reason.is_empty()
    {
        return String::from_utf8_lossy(&reason.reason)
            .chars()
            .take(256)
            .collect();
    }
    "Connection to the meeting host was lost".into()
}
