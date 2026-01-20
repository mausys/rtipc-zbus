use std::{error::Error, future::pending, os::fd::AsRawFd, os::fd::RawFd};

use tokio::io::{Interest, unix::AsyncFd};

use zbus::{connection, fdo::Error as ZBusError, interface, zvariant};

use rtipc::{ChannelVector, ConsumeResult, Consumer, ProduceTryResult, Producer, VectorResource};

use rtipc_zbus::{
    ChannelConfigBus, CommandId, MsgCommand, MsgEvent, MsgResponse, zbus_into_rtipc_vector_config,
};

struct Server {
    command: Consumer<MsgCommand>,
    response: Producer<MsgResponse>,
    event: Producer<MsgEvent>,
}

fn print_vector(vec: &ChannelVector) {
    let vec_info = str::from_utf8(vec.info()).unwrap();
    let cmd_info = str::from_utf8(vec.consumer_info(0).unwrap()).unwrap();
    let rsp_info = str::from_utf8(vec.producer_info(0).unwrap()).unwrap();
    let evt_info = str::from_utf8(vec.producer_info(1).unwrap()).unwrap();
    println!(
        "server received request vec={} cmd={} rsp={} evt={}",
        vec_info, cmd_info, rsp_info, evt_info
    );
}

impl Server {
    pub fn new(mut vec: ChannelVector) -> Self {
        print_vector(&vec);
        let command = vec.take_consumer(0).unwrap();
        let response = vec.take_producer(0).unwrap();
        let event = vec.take_producer(1).unwrap();

        Self {
            command,
            response,
            event,
        }
    }

    fn raw_fd(&self) -> Option<RawFd> {
        self.command.eventfd().map(|fd| fd.as_raw_fd())
    }

    fn process_cmd(&mut self) -> bool {
        match self.command.pop() {
            ConsumeResult::QueueError => panic!(),
            ConsumeResult::NoMessage => return false,
            ConsumeResult::NoNewMessage => return false,
            ConsumeResult::Success => {}
            ConsumeResult::SuccessMessagesDiscarded => {}
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
            } else if self.event.try_push() == ProduceTryResult::QueueFull {
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

    async fn connect(
        &mut self,
        shmfd_bus: zvariant::OwnedFd,
        consumers_zbus: Vec<ChannelConfigBus>,
        consumer_eventfds_bus: Vec<zvariant::OwnedFd>,
        producers_zbus: Vec<ChannelConfigBus>,
        producer_eventfds_bus: Vec<zvariant::OwnedFd>,
        info: Vec<u8>,
    ) -> Result<(), ZBusError> {
        let config = zbus_into_rtipc_vector_config(consumers_zbus, producers_zbus, info)?;

        let shmfd = shmfd_bus.into();

        let consumer_eventfds = consumer_eventfds_bus
            .into_iter()
            .map(|fd| fd.into())
            .collect();
        let producer_eventfds = producer_eventfds_bus
            .into_iter()
            .map(|fd| fd.into())
            .collect();

        let resource = VectorResource::new(&config, shmfd, consumer_eventfds, producer_eventfds)
            .map_err(|_| ZBusError::InvalidArgs(String::from("VectorResource failed")))?;

        let vec = ChannelVector::new(resource)
            .map_err(|_| ZBusError::InvalidArgs(String::from("ChannelVector filed")))?;

        let mut server = Server::new(vec);

        tokio::spawn(async move {
            let fd = server.raw_fd().unwrap();
            let notify = AsyncFd::new(fd).unwrap();
            let mut run = true;
            while run {
                let guard = notify.ready(Interest::READABLE).await.unwrap();
                if guard.ready().is_readable() {
                    run = server.process_cmd();
                }
            }
        });

        Ok(())
    }
}

// Although we use `tokio` here, you can use any async runtime of choice.
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
