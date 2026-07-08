use std::{error::Error, future::pending};

use zbus::{connection, fdo::Error as ZBusError, interface, zvariant};

use rtipc::{ChannelGroup, Consumer, EventFd, PopResult, Producer, TryPushResult};

use rtipc_zbus::{AsyncEventFd, CommandId, MsgCommand, MsgEvent, MsgResponse};

struct Server {
    command: Consumer<MsgCommand>,
    response: Producer<MsgResponse>,
    event: Producer<MsgEvent>,
}

fn print_group(grp: &ChannelGroup) {
    let attr = grp.get_attr();
    let grp_info = str::from_utf8(attr.info.iter().as_slice()).unwrap();
    let cmd_info = str::from_utf8(&attr.consumers.get(0).unwrap().info).unwrap();
    let rsp_info = str::from_utf8(&attr.producers.get(0).unwrap().info).unwrap();
    let evt_info = str::from_utf8(&attr.producers.get(1).unwrap().info).unwrap();
    println!(
        "server received request grp={} cmd={} rsp={} evt={}",
        grp_info, cmd_info, rsp_info, evt_info
    );
}

impl Server {
    pub fn new(mut grp: ChannelGroup) -> Self {
        print_group(&grp);
        let command = grp.take_consumer(0).unwrap();
        let response = grp.take_producer(0).unwrap();
        let event = grp.take_producer(1).unwrap();

        Self {
            command,
            response,
            event,
        }
    }

    fn take_eventfd(&mut self) -> Option<EventFd> {
        self.command.take_eventfd()
    }

    fn process_cmd(&mut self) -> bool {
        match self.command.pop() {
            PopResult::QueueError => panic!(),
            PopResult::NoMessage => return false,
            PopResult::NoNewMessage => return false,
            PopResult::Success => {}
            PopResult::SuccessMessagesDiscarded => {}
        };
        let mut run = true;
        let cmd = self.command.current_message().unwrap();
        self.response.current_message().id = cmd.id;
        let args: [i32; 3] = cmd.args;
        println!("server received command: {}", cmd);

        let cmdid: CommandId = unsafe { ::std::mem::transmute(cmd.id) };
        self.response.current_message().result = match cmdid {
            CommandId::Hello => 0,
            CommandId::Stop => {
                run = false;
                0
            }
            CommandId::SendEvent => self.send_events(args[0] as u32, args[1] as u32, args[2] != 0),
            CommandId::Div => {
                let (err, res) = self.div(args[0], args[1]);
                self.response.current_message().data = res;
                err
            }
        };
        self.response.force_push();
        run
    }
    fn send_events(&mut self, id: u32, num: u32, force: bool) -> i32 {
        for i in 0..num {
            let event = self.event.current_message();
            event.id = id;
            event.nr = i;
            if force {
                self.event.force_push();
            } else if self.event.try_push() == TryPushResult::QueueFull {
                return i as i32;
            }
        }
        num as i32
    }
    fn div(&mut self, a: i32, b: i32) -> (i32, i32) {
        if b == 0 { (-1, 0) } else { (0, a / b) }
    }
}

struct ServerInterface {}

#[interface(name = "org.rtipc.server")]
impl ServerInterface {
    // Can be `async` as well.
    // a producer on the client side is a consumer on the server side
    // and vice versa
    async fn connect(
        &mut self,
        request: Vec<u8>,
        fds: Vec<zvariant::OwnedFd>,
    ) -> Result<(), ZBusError> {
        let fdsq = fds.into_iter().map(|fd| fd.into()).collect();

        let grp = ChannelGroup::deserialize(request.as_slice(), fdsq).map_err(|_| {
            ZBusError::InvalidArgs(String::from("ChannelGroup::deserialize failed"))
        })?;

        let mut server = Server::new(grp);

        tokio::task::spawn(async move {
            let fd = server.take_eventfd().unwrap();
            let async_fd = AsyncEventFd::new(fd).unwrap();

            let mut run = true;
            while run {
                async_fd
                    .await_event()
                    .await
                    .inspect_err(|e| println!("await_event error {e}"))
                    .unwrap();

                run = server.process_cmd();
            }
        });

        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let greeter = ServerInterface {};
    let _conn = connection::Builder::session()?
        .name("org.rtipc.server")?
        .serve_at("/org/rtipc/server", greeter)?
        .build()
        .await?;

    // Do other things or go to wait forever
    pending::<()>().await;

    Ok(())
}
