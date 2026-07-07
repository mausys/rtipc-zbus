use std::fmt;

use std::io;
use std::num::NonZeroUsize;

use serde::{Deserialize, Serialize};

use tokio::io::Error;
use tokio::io::unix::AsyncFd;

use zbus::fdo::Error as ZBusError;

use rtipc::{ChannelAttr, EventFd, GroupAttr, QueueAttr};

pub struct AsyncEventFd {
    fd: AsyncFd<EventFd>,
}

impl AsyncEventFd {
    pub fn new(fd: EventFd) -> io::Result<Self> {
        Ok(Self {
            fd: AsyncFd::new(fd)?,
        })
    }

    pub async fn await_event(&self) -> io::Result<u64> {
        loop {
            let mut guard = self.fd.readable().await?;

            match guard.try_io(|fd| {
                fd.get_ref()
                    .read()
                    .map_err(|e| Error::from_raw_os_error(e as i32))
            }) {
                Ok(result) => return result,
                Err(_would_block) => continue,
            }
        }
    }
}

#[derive(zvariant::Type, Debug, Serialize, Deserialize)]
pub struct ChannelAttrBus {
    pub additonal_messages: u32,
    pub message_size: u32,
    pub eventfd: bool,
    pub info: Vec<u8>,
}

impl ChannelAttrBus {
    fn from_rtipc_attr(attr: &ChannelAttr) -> Self {
        Self {
            additonal_messages: attr.queue.additional_messages as u32,
            message_size: attr.queue.message_size.get() as u32,
            eventfd: attr.eventfd,
            info: attr.info.clone(),
        }
    }

    fn into_rtipc_attr(self) -> Result<ChannelAttr, ZBusError> {
        let message_size = NonZeroUsize::new(self.message_size as usize).ok_or(
            ZBusError::InvalidArgs(String::from("message_size can't be zero")),
        )?;
        Ok(ChannelAttr {
            queue: QueueAttr {
                additional_messages: self.additonal_messages as usize,
                message_size,
            },
            info: self.info,
            eventfd: self.eventfd,
        })
    }
}

fn zbus_into_rtipc_attr(channels_bus: Vec<ChannelAttrBus>) -> Result<Vec<ChannelAttr>, ZBusError> {
    let channels: Result<Vec<ChannelAttr>, ZBusError> = channels_bus
        .into_iter()
        .map(|c| c.into_rtipc_attr())
        .collect();
    channels
}

pub fn zbus_into_rtipc_group_attr(
    consumers_zbus: Vec<ChannelAttrBus>,
    producers_zbus: Vec<ChannelAttrBus>,
    info: Vec<u8>,
) -> Result<GroupAttr, ZBusError> {
    let consumers = zbus_into_rtipc_attr(consumers_zbus)?;
    let producers = zbus_into_rtipc_attr(producers_zbus)?;

    Ok(GroupAttr {
        consumers,
        producers,
        info,
    })
}

pub fn rtipc_into_zbus_attr(attrs: &[ChannelAttr]) -> Vec<ChannelAttrBus> {
    attrs.iter().map(ChannelAttrBus::from_rtipc_attr).collect()
}

#[repr(u32)]
#[derive(Copy, Clone, Debug)]
pub enum CommandId {
    Hello = 1,
    Stop = 2,
    SendEvent = 3,
    Div = 4,
}

#[derive(Copy, Clone, Debug)]
pub struct MsgCommand {
    pub id: u32,
    pub args: [i32; 3],
}

#[derive(Copy, Clone, Debug)]
pub struct MsgResponse {
    pub id: u32,
    pub result: i32,
    pub data: i32,
}

#[derive(Copy, Clone, Debug)]
pub struct MsgEvent {
    pub id: u32,
    pub nr: u32,
}

impl fmt::Display for MsgCommand {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        writeln!(f, "id: {}", self.id)?;
        for (idx, arg) in self.args.iter().enumerate() {
            writeln!(f, "\targ[{}]: {}", idx, arg)?
        }
        Ok(())
    }
}

impl fmt::Display for MsgResponse {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        writeln!(
            f,
            "id: {}\n\tresult: {}\n\tdata: {}",
            self.id, self.result, self.data
        )
    }
}

impl fmt::Display for MsgEvent {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        writeln!(f, "id: {}\n\tnr: {}", self.id, self.nr)
    }
}
