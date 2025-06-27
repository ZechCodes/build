"""Mock PTY manager for terminal simulation during development."""

import asyncio
import time
import uuid
from typing import Dict, Optional, List, AsyncIterator
import structlog

logger = structlog.get_logger(__name__)


class MockTerminalSession:
    """Mock terminal session for development."""
    
    def __init__(self, session_id: str, vm_id: str, user_id: str):
        self.session_id = session_id
        self.vm_id = vm_id
        self.user_id = user_id
        self.created_at = time.time()
        self.last_activity = time.time()
        self.history: List[str] = []
        self.rows = 24
        self.cols = 80
        self.output_queue: asyncio.Queue = asyncio.Queue()
        self.active = True
        
        # Add welcome message
        welcome_msg = f"Welcome to Build Platform Terminal\n"
        welcome_msg += f"Session ID: {session_id}\n"
        welcome_msg += f"VM ID: {vm_id}\n"
        welcome_msg += f"Type 'help' for available commands\n"
        welcome_msg += f"user@build-vm:~$ "
        
        self.history.append(welcome_msg)
        asyncio.create_task(self._send_output(welcome_msg.encode()))
    
    async def _send_output(self, data: bytes):
        """Send output data to the queue."""
        try:
            await self.output_queue.put(data)
        except:
            pass  # Queue might be closed
    
    async def process_input(self, data: bytes):
        """Process input from the terminal."""
        try:
            input_str = data.decode('utf-8', errors='replace')
            self.last_activity = time.time()
            
            # Echo the input
            await self._send_output(data)
            
            # Process commands
            if input_str.strip():
                response = await self._process_command(input_str.strip())
                if response:
                    response_with_prompt = f"\n{response}\nuser@build-vm:~$ "
                    await self._send_output(response_with_prompt.encode())
                    self.history.append(response_with_prompt)
            elif '\r' in input_str or '\n' in input_str:
                # Just enter pressed, show new prompt
                prompt = "\nuser@build-vm:~$ "
                await self._send_output(prompt.encode())
                self.history.append(prompt)
                
        except Exception as e:
            logger.error("Error processing input", 
                        session_id=self.session_id,
                        error=str(e))
    
    async def _process_command(self, command: str) -> str:
        """Process a terminal command and return response."""
        command = command.strip()
        
        if command == "help":
            return """Available commands:
  help     - Show this help message
  date     - Show current date and time
  whoami   - Show current user
  pwd      - Show current directory
  ls       - List directory contents (simulated)
  echo     - Echo text back
  clear    - Clear terminal (sends clear sequence)
  exit     - End terminal session
  uptime   - Show system uptime (simulated)
  ps       - Show running processes (simulated)"""
        
        elif command == "date":
            return time.strftime("%a %b %d %H:%M:%S %Z %Y")
        
        elif command == "whoami":
            return "user"
        
        elif command == "pwd":
            return "/home/user"
        
        elif command == "ls":
            return "documents  downloads  projects  build-config.yml"
        
        elif command.startswith("echo "):
            return command[5:]  # Return everything after "echo "
        
        elif command == "clear":
            # Send ANSI clear screen sequence
            asyncio.create_task(self._send_output(b"\033[2J\033[H"))
            return ""
        
        elif command == "exit":
            self.active = False
            return "Session ended. Connection will be closed."
        
        elif command == "uptime":
            uptime_seconds = time.time() - self.created_at
            hours = int(uptime_seconds // 3600)
            minutes = int((uptime_seconds % 3600) // 60)
            return f"up {hours}:{minutes:02d}, 1 user, load average: 0.15, 0.10, 0.05"
        
        elif command == "ps":
            return """  PID TTY          TIME CMD
    1 ?        00:00:01 systemd
   42 pts/0    00:00:00 bash
  123 pts/0    00:00:00 python
  456 pts/0    00:00:00 ps"""
        
        else:
            return f"bash: {command}: command not found"
    
    async def resize(self, rows: int, cols: int):
        """Handle terminal resize."""
        self.rows = rows
        self.cols = cols
        self.last_activity = time.time()
        
        # Send resize acknowledgment
        response = f"Terminal resized to {cols}x{rows}"
        logger.info("Terminal resized", 
                   session_id=self.session_id,
                   rows=rows, cols=cols)


class MockPTYManager:
    """Mock PTY manager for terminal simulation."""
    
    def __init__(self):
        self.sessions: Dict[str, MockTerminalSession] = {}
        self.cleanup_task: Optional[asyncio.Task] = None
        self.cleanup_interval = 300  # 5 minutes
        self.session_timeout = 3600  # 1 hour
    
    async def start(self):
        """Start the PTY manager."""
        self.cleanup_task = asyncio.create_task(self._cleanup_loop())
        logger.info("Mock PTY manager started")
    
    async def stop(self):
        """Stop the PTY manager."""
        if self.cleanup_task:
            self.cleanup_task.cancel()
        
        # Close all sessions
        for session in self.sessions.values():
            session.active = False
        
        self.sessions.clear()
        logger.info("Mock PTY manager stopped")
    
    async def create_session(self, vm_id: str, user_id: str) -> str:
        """Create a new terminal session."""
        session_id = str(uuid.uuid4())
        session = MockTerminalSession(session_id, vm_id, user_id)
        self.sessions[session_id] = session
        
        logger.info("Mock terminal session created",
                   session_id=session_id,
                   vm_id=vm_id,
                   user_id=user_id)
        
        return session_id
    
    async def session_exists(self, session_id: str) -> bool:
        """Check if session exists."""
        return session_id in self.sessions
    
    async def send_input(self, session_id: str, data: bytes):
        """Send input to terminal session."""
        session = self.sessions.get(session_id)
        if session and session.active:
            await session.process_input(data)
        else:
            logger.warning("Input sent to unknown session", session_id=session_id)
    
    async def resize_terminal(self, session_id: str, rows: int, cols: int):
        """Resize terminal session."""
        session = self.sessions.get(session_id)
        if session and session.active:
            await session.resize(rows, cols)
        else:
            logger.warning("Resize sent to unknown session", session_id=session_id)
    
    async def get_session_history(self, session_id: str) -> Optional[List[str]]:
        """Get session history for recovery."""
        session = self.sessions.get(session_id)
        if session:
            return session.history.copy()
        return None
    
    async def get_output_stream(self, session_id: str) -> AsyncIterator[bytes]:
        """Get output stream for a session."""
        session = self.sessions.get(session_id)
        if not session:
            logger.warning("Output stream requested for unknown session", 
                          session_id=session_id)
            return
        
        logger.debug("Starting output stream", session_id=session_id)
        
        try:
            while session.active:
                try:
                    # Wait for output data with timeout
                    output_data = await asyncio.wait_for(
                        session.output_queue.get(), 
                        timeout=1.0
                    )
                    yield output_data
                    
                except asyncio.TimeoutError:
                    # Timeout is normal, just continue
                    continue
                except Exception as e:
                    logger.error("Error in output stream", 
                                session_id=session_id,
                                error=str(e))
                    break
                    
        except Exception as e:
            logger.error("Output stream error", 
                        session_id=session_id,
                        error=str(e))
        finally:
            logger.debug("Output stream ended", session_id=session_id)
    
    async def end_session(self, session_id: str):
        """End a terminal session."""
        session = self.sessions.get(session_id)
        if session:
            session.active = False
            del self.sessions[session_id]
            logger.info("Mock terminal session ended", session_id=session_id)
    
    async def _cleanup_loop(self):
        """Cleanup inactive sessions."""
        while True:
            try:
                await asyncio.sleep(self.cleanup_interval)
                current_time = time.time()
                
                # Find expired sessions
                expired_sessions = []
                for session_id, session in self.sessions.items():
                    if (not session.active or 
                        current_time - session.last_activity > self.session_timeout):
                        expired_sessions.append(session_id)
                
                # Clean up expired sessions
                for session_id in expired_sessions:
                    await self.end_session(session_id)
                    logger.info("Expired session cleaned up", session_id=session_id)
                    
            except asyncio.CancelledError:
                break
            except Exception as e:
                logger.error("Session cleanup error", error=str(e))