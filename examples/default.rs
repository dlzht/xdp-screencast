use xdp_screencast::screencast::ScreenCast;

#[tokio::main(flavor = "current_thread")]
async fn main() {
  let mut screencast = ScreenCast::default();
  let result = screencast.screencast().await;
  println!("{:?} {:?}", result, screencast.get_selected_sources());
}