//! Integration tests of the daemon: the contract of every route, authentication and the socket, the event stream,
//! the OpenAPI document and the simulation. One test binary (they all link the whole server).

mod auth;
mod common;
mod contract;
mod electron_api;
mod openapi;
mod real_service;
mod sim;
mod sse;
mod views;
