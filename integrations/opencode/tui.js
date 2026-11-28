import { startPublisher } from "./v2/publisher.js"

export default { id: "rozi", setup(context) { return startPublisher(context) } }
