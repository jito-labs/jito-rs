## Jito-Client
This repository contains code to interact with the Solana network using Jito.
It is intended to be used by developers looking to leverage Jito bundles when 
applicable (i.e. when a jito-solana validator is the leader) otherwise use the normal
transaction send pipeline. This way developers can focus on their core business logic
without worrying about keeping track of when to send a bundle vs. a normal transaction.